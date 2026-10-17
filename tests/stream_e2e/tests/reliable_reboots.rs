//! A reliable stream kept in a logring track, end to end: the device runs the generated async server with
//! `Reliable<TrackStore>` on a simulated card, the host the real `wire_weaver_client` event loop over the in-process
//! link. The device reboots (and loses power with records still in its stage buffer) while the host is away; after
//! every boot the host resumes with its cursor and gets the backlog from the card as the backfill run, acknowledges
//! it, and ends up with every sample that reached the card exactly once.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use logring::tests_support::block_on;
use logring::{
    Config, FullPolicy, FullVersion, Logring, SessionInfo, TrackDesc, TrackStore, UNib32,
};
use logring_stream_api::Sample;
use tests_common::TestDevice;
use wire_weaver::prelude::*;
use wire_weaver::ww_version::Version;
use wire_weaver_client::StreamEvent;
use ww_stream::StreamCursor;
use ww_stream::store::LogMedium;
use ww_stream::store::log::{SimError, SimSd};
use ww_stream::tx::Reliable;

const BUF: usize = 1024;
const TRACKS: usize = 4;
type Lr = Logring<'static, Card, BUF, TRACKS>;
type Shared = Mutex<CriticalSectionRawMutex, Lr>;
type Store = TrackStore<'static, 'static, CriticalSectionRawMutex, Card, BUF, TRACKS>;

/// A simulated card that survives the device: a power cut makes the old device's handle fail every access.
#[derive(Clone)]
struct Card {
    sim: Arc<StdMutex<SimSd>>,
    power: Arc<AtomicU32>,
    on: u32,
}

#[derive(Debug)]
#[allow(dead_code)]
enum CardError {
    PoweredOff,
    Sim(SimError),
}

impl Card {
    fn new(size: usize) -> Self {
        Card {
            sim: Arc::new(StdMutex::new(SimSd::new(size, 41))),
            power: Arc::new(AtomicU32::new(0)),
            on: 0,
        }
    }

    /// Cut the power of the device using this card: whatever it hadn't written is gone.
    fn cut(&self) {
        self.power.fetch_add(1, Ordering::SeqCst);
    }

    /// The card in the next device.
    fn power_on(&self) -> Card {
        Card {
            on: self.power.load(Ordering::SeqCst),
            ..self.clone()
        }
    }

    fn check(&self) -> Result<(), CardError> {
        if self.power.load(Ordering::SeqCst) == self.on {
            Ok(())
        } else {
            Err(CardError::PoweredOff)
        }
    }
}

impl LogMedium for Card {
    type Error = CardError;
    const WRITE_SIZE: usize = 512;
    const ERASE_SIZE: usize = 512;
    const NEEDS_ERASE: bool = false;

    fn capacity(&self) -> u64 {
        self.sim.lock().unwrap().capacity()
    }

    async fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), CardError> {
        self.check()?;
        let mut sim = self.sim.lock().unwrap();
        block_on(sim.read(offset, buf)).map_err(CardError::Sim)
    }

    async fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), CardError> {
        self.check()?;
        let mut sim = self.sim.lock().unwrap();
        block_on(sim.write(offset, data)).map_err(CardError::Sim)
    }

    async fn erase(&mut self, from: u64, to: u64) -> Result<(), CardError> {
        self.check()?;
        let mut sim = self.sim.lock().unwrap();
        block_on(sim.erase(from, to)).map_err(CardError::Sim)
    }
}

static TRACK_DESC: [TrackDesc<'static>; 1] = [TrackDesc::new(&[UNib32(0)])];

fn config() -> Config {
    Config {
        segment_size: 8192,
        meta_segments: 2,
        flush_bytes: 2048,
        flush_interval_ms: 20,
        full: FullPolicy::DropOldest,
        seed: 1,
        ..Config::default()
    }
}

fn mount(card: Card) -> &'static Shared {
    let info = SessionInfo {
        api: FullVersion::new("logring_stream_api", Version::new(0, 0, 0)),
        device_id: "e2e",
        fw: "test",
        hw: "sim",
        tracks: &TRACK_DESC,
        bundle: None,
    };
    let lr = block_on(Lr::mount_leaked(card, config(), info)).unwrap();
    Box::leak(Box::new(Mutex::new(lr)))
}

mod server {
    use super::*;
    use tests_common::TestProcessEventsAsync;

    pub struct Server {
        pub lr: &'static Shared,
        pub samples_stream: Reliable<Store>,
        pub stream_sessions: u32,
        pub next_value: u32,
        pub buf: [u8; 64],
    }

    impl Server {
        /// Boot: the stream continues the session the card holds.
        pub fn new(lr: &'static Shared) -> Self {
            let store = block_on(TrackStore::new(lr, 1)).unwrap();
            let mut samples_stream = Reliable::new(store);
            samples_stream.continue_stored_session();
            Server {
                lr,
                next_value: samples_stream.next_pos() as u32,
                samples_stream,
                stream_sessions: 0,
                buf: [0; 64],
            }
        }

        async fn samples(&mut self, _cx: &mut Context<'_, impl EventOut>) -> RpcResult<()> {
            RpcResult::Ready(())
        }

        async fn close_samples(&mut self, _cx: &mut Context<'_, impl EventOut>) {}

        async fn stream_samples(&mut self) -> &mut Reliable<Store> {
            &mut self.samples_stream
        }

        async fn produce(
            &mut self,
            _cx: &mut Context<'_, impl EventOut>,
            count: u32,
        ) -> RpcResult<()> {
            let ser = api_impl::stream_data_ser();
            for _ in 0..count {
                let sample = Sample {
                    value: self.next_value,
                };
                self.next_value += 1;
                // after a power cut the card fails: nothing to do about it here
                let _ = ser
                    .samples_push(&mut self.samples_stream, &sample, 1, None, &mut self.buf)
                    .await;
            }
            RpcResult::Ready(())
        }

        fn new_stream_session(&mut self) -> u32 {
            self.stream_sessions = self.stream_sessions.wrapping_add(1);
            self.stream_sessions
        }
    }

    pub mod api_impl {
        wire_weaver::ww_codegen!(
            logring_stream_api :: LogringStream for super::Server,
            server = true, no_alloc = true, use_async = true,
            method_model = "_=immediate",
            property_model = "_=get_set",
        );
    }

    impl TestProcessEventsAsync for Server {
        type Medium = ();

        async fn process_request_bytes<'a>(
            &mut self,
            bytes: &[u8],
            scratch: &'a mut [u8],
            out: &mut impl EventOut,
            medium: (),
        ) -> Result<&'a [u8], ShrinkWrapError> {
            self.process_request_bytes(bytes, scratch, out, medium)
                .await
        }

        async fn poll(&mut self, out: &mut impl EventOut, now_ms: u64) {
            // the flush interval makes logged samples durable, and only then they are sent
            let _ = self.lr.lock().await.flush_if_due(now_ms).await;
            let mut buf = [0u8; 256];
            _ = self.poll_streams(out, now_ms, &mut buf, 4096).await;
        }
    }
}

mod client {
    use wire_weaver_client::{ClientConfig, Commander, WwClient};

    pub struct Client {
        pub cmd: Commander,
    }

    impl WwClient for Client {
        fn default_config() -> ClientConfig {
            ClientConfig::default()
        }

        fn from_cmd(cmd: Commander) -> Self {
            Self { cmd }
        }
    }

    mod api_client {
        wire_weaver::ww_codegen!(
            logring_stream_api :: LogringStream for super::Client,
            client = "std_client",
        );
    }
}

use client::Client;

fn start(path: &str, server: server::Server) -> TestDevice {
    let _ = tracing_subscriber::fmt::try_init();
    tests_common::start_device_async(
        path,
        server,
        logring_stream_api::LOGRING_STREAM_FULL_GID,
        server::api_impl::api_hash(),
    )
}

async fn connect(device: &TestDevice) -> Client {
    Client::config(|c| c.in_process_path(device.path()))
        .connect()
        .await
        .expect("connect")
}

/// Take events until the host has every position below `until`; every item's value is its position, none
/// arrives twice, there are no gaps.
async fn collect(
    samples: &mut wire_weaver_client::PositionedStream<Sample>,
    seen: &mut BTreeMap<u64, u32>,
    until: u64,
) {
    while seen.range(..until).count() < until as usize {
        let ev = tokio::time::timeout(Duration::from_secs(3), samples.recv_event()).await;
        let ev = ev
            .unwrap_or_else(|_| panic!("stuck: have {} of {until}", seen.range(..until).count()))
            .expect("stream open");
        match ev {
            StreamEvent::Item { pos, value, .. } => {
                assert_eq!(value.value as u64, pos, "the value is the position");
                assert!(seen.insert(pos, value.value).is_none(), "{pos} twice");
            }
            StreamEvent::Resumed { .. } | StreamEvent::BackfillDone => {}
            ev => panic!("{ev:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reliable_stream_backfills_from_the_card_across_reboots() {
    let card = Card::new(8192 * (1 + 2 + 48));
    let mut seen = BTreeMap::new();
    let mut cursor: Option<StreamCursor> = None;
    for boot in 0..4 {
        let lr = mount(card.power_on());
        let server = server::Server::new(lr);
        // what reached the card before the last power cut: the stream goes on from there
        let durable = server.samples_stream.next_pos();
        if boot > 0 {
            assert!(
                durable >= seen.len() as u64,
                "boot {boot}: positions continue"
            );
            assert_eq!(
                server.samples_stream.session(),
                Some(1),
                "one stream session"
            );
        }
        let device = start(&format!("logring_e2e/boot{boot}"), server);
        let mut client = connect(&device).await;
        let mut samples = match cursor {
            None => client.samples().call().await.unwrap(),
            Some(c) => client.samples_resume(c).call().await.unwrap(),
        };
        // the backlog of the boot before (backfill) and live samples of this one
        client.produce(150).call().await.unwrap();
        collect(&mut samples, &mut seen, durable + 150).await;
        cursor = samples.cursor();
        assert_eq!(cursor.unwrap().pos.0, durable + 150);

        // the host goes; the device logs on, some of it reaches the card, then the power goes
        samples.close().await.unwrap();
        drop(samples);
        client.produce(80).call().await.unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;
        client.produce(7).call().await.unwrap();
        card.cut();
        drop(client);
        drop(device);
    }
    // the last boot's backlog, and the store drains once it is acknowledged
    let lr = mount(card.power_on());
    let server = server::Server::new(lr);
    let durable = server.samples_stream.next_pos();
    let device = start("logring_e2e/last", server);
    let client = connect(&device).await;
    let mut samples = client.samples_resume(cursor.unwrap()).call().await.unwrap();
    collect(&mut samples, &mut seen, durable).await;
    assert_eq!(
        seen.len() as u64,
        durable,
        "every sample that reached the card, once"
    );
    assert!(durable >= 4 * 230, "{durable}");
    let start = std::time::Instant::now();
    loop {
        let status = client.samples_status().read().await.unwrap();
        assert_eq!(status.session, Some(UVlq32(1)));
        if status.oldest.is_none() {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "never acknowledged: {status:?}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
