//! Time-conversion helpers for the AAudio backend.

extern crate ndk;

use crate::{
    timestamp::{
        map_output_timestamp, stream_instant_from_anchor, OutputTimestampFallbackReason,
        OutputTimestampMapping,
    },
    Error, ErrorKind, StreamInstant,
};

/// Returns a [`StreamInstant`] for the current moment.
pub fn now_stream_instant() -> StreamInstant {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let res = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    assert_eq!(res, 0, "clock_gettime(CLOCK_MONOTONIC) failed");
    StreamInstant::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

/// Returns the [`StreamInstant`] for when the first frame of the current output callback will
/// be presented at the DAC.
pub fn output_stream_instant(
    stream: &ndk::audio::AudioStream,
    sample_rate: u32,
    last_device_presentation: Option<StreamInstant>,
    fallback: StreamInstant,
) -> OutputTimestampMapping {
    let anchor = stream
        .timestamp(ndk::audio::Clockid::Monotonic)
        .map(|ts| (ts.frame_position, ts.time_nanoseconds))
        .map_err(|error| match error {
            ndk::audio::AudioError::Unimplemented => OutputTimestampFallbackReason::Unsupported,
            _ => OutputTimestampFallbackReason::Unavailable,
        });
    map_output_timestamp(
        anchor,
        stream.frames_written(),
        sample_rate,
        true,
        last_device_presentation,
        fallback,
    )
}

/// Returns the [`StreamInstant`] for when the first frame of the current input callback was
/// captured at the ADC.
pub fn input_stream_instant(stream: &ndk::audio::AudioStream, sample_rate: u32) -> StreamInstant {
    match stream.timestamp(ndk::audio::Clockid::Monotonic) {
        Ok(ts) => stream_instant_from_anchor(
            ts.frame_position,
            ts.time_nanoseconds,
            stream.frames_read(),
            sample_rate,
        )
        .unwrap_or_else(now_stream_instant),
        Err(_) => now_stream_instant(),
    }
}

impl From<ndk::audio::AudioError> for Error {
    fn from(error: ndk::audio::AudioError) -> Self {
        use ndk::audio::AudioError::*;
        match error {
            Disconnected | Unavailable | NoService | InvalidHandle => {
                Error::with_message(ErrorKind::DeviceNotAvailable, error.to_string())
            }
            NoFreeHandles | NoMemory => {
                Error::with_message(ErrorKind::ResourceExhausted, error.to_string())
            }
            WouldBlock | Timeout => Error::with_message(ErrorKind::DeviceBusy, error.to_string()),
            InvalidFormat | InvalidRate => {
                Error::with_message(ErrorKind::UnsupportedConfig, error.to_string())
            }
            IllegalArgument | Null | OutOfRange => {
                Error::with_message(ErrorKind::InvalidInput, error.to_string())
            }
            Internal | InvalidState => {
                Error::with_message(ErrorKind::StreamInvalidated, error.to_string())
            }
            Unimplemented => {
                Error::with_message(ErrorKind::UnsupportedOperation, error.to_string())
            }
            _ => Error::with_message(ErrorKind::BackendError, error.to_string()),
        }
    }
}
