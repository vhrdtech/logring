//! [run]: a stretch of the drive's endurance bench, the same items every time, at any length.
//!
//! The drive boots, logs that it is up and runs the motor from the first second on, in a recording. Every second it
//! records the battery voltage and every two seconds a log line; frames of the vehicle bus arrive at
//! [Bench::can_hz]. With [Bench::fault_at_s] it faults on overcurrent at that second: it marks the moment, stops the
//! motor and the recording. Without a fault it stops the motor and the recording at the end.

use crate::api::{Level, Mode};
use crate::drive::{Drive, Error, marker, reason, trigger};
use logring::LogMedium;

/// What a run of the bench covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bench {
    /// Seconds since boot the run lasts.
    pub seconds: u32,
    /// Frames per second on the vehicle bus.
    pub can_hz: u32,
    /// The second of the overcurrent fault, if the run has one.
    pub fault_at_s: Option<u32>,
}

impl Bench {
    /// The drive's first minute on the bench: a thousand frames a second, no fault.
    pub const FIRST_MINUTE: Bench = Bench {
        seconds: 60,
        can_hz: 1000,
        fault_at_s: None,
    };
}

/// What a run recorded, per track.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    pub log: u32,
    pub mode: u32,
    pub can0: u32,
    pub vbat: u32,
    /// Recordings started and markers placed.
    pub recordings: u32,
    pub markers: u32,
    /// Flushes the tick wrote; flushes the stage buffer or an `Immediate` track forced are not counted.
    pub ticked: u32,
    /// The bench time the run reached, in ms since boot.
    pub t_ms: u64,
}

/// The phase current of the overcurrent fault, in mA.
pub const OVERCURRENT_MA: i64 = 41_500;

/// A frame's payload: the motor's speed and a counter, as a drive would send them.
fn frame(n: u32) -> [u8; 8] {
    let rpm = 3000 + (n % 64) as u16;
    let [a, b] = rpm.to_le_bytes();
    let [c, d, e, f] = n.to_le_bytes();
    [a, b, c, d, e, f, 0, 0]
}

/// Run the bench on a mounted `drive`, then write what is left to the card.
pub async fn run<M: LogMedium>(
    drive: &mut Drive<'_, M>,
    bench: Bench,
) -> Result<Totals, Error<M::Error>> {
    let mut n = Totals::default();
    run_counting(drive, bench, &mut n).await?;
    Ok(n)
}

/// [run], counting into `n` as it goes: after an error, such as a power cut, `n` says what the drive had pushed.
pub async fn run_counting<M: LogMedium>(
    drive: &mut Drive<'_, M>,
    bench: Bench,
    n: &mut Totals,
) -> Result<(), Error<M::Error>> {
    drive.log(0, Level::Info, "drive fw 1.4.0 up").await?;
    n.log += 1;
    drive.mode(0, Mode::Idle).await?;
    n.mode += 1;
    let mut frames: u32 = 0;
    let mut faulted = false;
    // one step of the loop is 100 ms of the bench
    for step in 0..bench.seconds * 10 {
        let t_ms = u64::from(step) * 100;
        n.t_ms = t_ms;
        let second = step / 10;
        if step % 10 == 0 {
            if second == 1 {
                drive.mode(t_ms * 1000, Mode::Run).await?;
                drive.recording_start(trigger::MOTOR_START).await?;
                n.mode += 1;
                n.recordings += 1;
            }
            if bench.fault_at_s == Some(second) {
                drive.mode(t_ms * 1000, Mode::Fault).await?;
                drive
                    .log(t_ms * 1000, Level::Error, "overcurrent on phase a")
                    .await?;
                drive
                    .marker(marker::OVERCURRENT, "phase a", Some(OVERCURRENT_MA))
                    .await?;
                drive.recording_stop(reason::FAULT).await?;
                n.mode += 1;
                n.log += 1;
                n.markers += 1;
                faulted = true;
            }
            drive
                .vbat(t_ms * 1000, 48_000 - (second % 200) as u16)
                .await?;
            n.vbat += 1;
            if second % 2 == 0 {
                drive.log(t_ms * 1000, Level::Info, "board at 41 C").await?;
                n.log += 1;
            }
        }
        if !faulted {
            let due = (u64::from(step) + 1) * u64::from(bench.can_hz) / 10;
            while u64::from(frames) < due {
                let t_us = u64::from(frames) * 1_000_000 / u64::from(bench.can_hz);
                drive.can_frame(t_us, 0x101, &frame(frames)).await?;
                frames += 1;
                n.can0 += 1;
            }
        }
        if drive.tick(t_ms).await? {
            n.ticked += 1;
        }
    }
    if !faulted && bench.seconds > 1 {
        let t_us = u64::from(bench.seconds) * 1_000_000;
        drive.mode(t_us, Mode::Idle).await?;
        drive.recording_stop(reason::MOTOR_STOP).await?;
        n.mode += 1;
    }
    drive.sync().await?;
    Ok(())
}
