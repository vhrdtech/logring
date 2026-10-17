//! API of the logring end-to-end stream test (`tests/stream_e2e`): a reliable stream whose store is a track of a
//! logring on a (simulated) card, and a method that makes the device log into it.
use wire_weaver::prelude::*;

#[ww_trait]
trait LogringStream {
    /// Samples logged on the device's card, kept until acknowledged; unit: sample
    #[stream(Sample, reliable)]
    fn samples();

    /// Device logs `count` samples, values counting on from the last one (the value is the position)
    fn produce(count: u32);
}

#[derive_shrink_wrap(
    borrowed,
    owned(feature = "std"),
    derive(Debug, PartialEq, Eq, Clone, Copy)
)]
pub struct Sample {
    pub value: u32,
}
