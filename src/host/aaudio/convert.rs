use std::convert::TryInto;
use std::os::raw::{c_int, c_long};

extern crate ndk;

use crate::{
    BackendSpecificError, BuildStreamError, PauseStreamError, PlayStreamError, StreamError,
    StreamInstant,
};

#[repr(C)]
struct Timespec {
    tv_sec: c_long,
    tv_nsec: c_long,
}

unsafe extern "C" {
    fn clock_gettime(clock_id: c_int, ts: *mut Timespec) -> c_int;
}

const CLOCK_MONOTONIC: c_int = 1;

fn stream_instant_from_nanos(total_nanos: i128) -> Option<StreamInstant> {
    let clamped_nanos = total_nanos.max(0);
    let secs = clamped_nanos / 1_000_000_000;
    if secs > i64::MAX as i128 {
        return None;
    }
    let nanos = (clamped_nanos % 1_000_000_000) as u32;
    Some(StreamInstant::new(secs as i64, nanos))
}

pub fn now_stream_instant() -> StreamInstant {
    let mut ts = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let result = unsafe { clock_gettime(CLOCK_MONOTONIC, &mut ts) };
    assert_eq!(result, 0, "clock_gettime(CLOCK_MONOTONIC) failed");
    StreamInstant::new(ts.tv_sec.try_into().unwrap(), ts.tv_nsec as u32)
}

fn stream_instant_from_anchor(
    anchor_frame: i64,
    anchor_nanos: i64,
    app_frame: i64,
    sample_rate: u32,
) -> Option<StreamInstant> {
    let offset_nanos =
        (app_frame as i128 - anchor_frame as i128) * 1_000_000_000 / sample_rate as i128;
    stream_instant_from_nanos(anchor_nanos as i128 + offset_nanos)
}

pub fn output_stream_instant(stream: &ndk::audio::AudioStream, sample_rate: u32) -> StreamInstant {
    match stream.timestamp(ndk::audio::Clockid::Monotonic) {
        Ok(ts) => stream_instant_from_anchor(
            ts.frame_position,
            ts.time_nanoseconds,
            stream.frames_written(),
            sample_rate,
        )
        .unwrap_or_else(now_stream_instant),
        Err(_) => now_stream_instant(),
    }
}

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

impl From<ndk::audio::AudioError> for StreamError {
    fn from(error: ndk::audio::AudioError) -> Self {
        use self::ndk::audio::AudioError::*;
        match error {
            Disconnected | Unavailable => Self::DeviceNotAvailable,
            e => (BackendSpecificError {
                description: e.to_string(),
            })
            .into(),
        }
    }
}

impl From<ndk::audio::AudioError> for PlayStreamError {
    fn from(error: ndk::audio::AudioError) -> Self {
        use self::ndk::audio::AudioError::*;
        match error {
            Disconnected | Unavailable => Self::DeviceNotAvailable,
            e => (BackendSpecificError {
                description: e.to_string(),
            })
            .into(),
        }
    }
}

impl From<ndk::audio::AudioError> for PauseStreamError {
    fn from(error: ndk::audio::AudioError) -> Self {
        use self::ndk::audio::AudioError::*;
        match error {
            Disconnected | Unavailable => Self::DeviceNotAvailable,
            e => (BackendSpecificError {
                description: e.to_string(),
            })
            .into(),
        }
    }
}

impl From<ndk::audio::AudioError> for BuildStreamError {
    fn from(error: ndk::audio::AudioError) -> Self {
        use self::ndk::audio::AudioError::*;
        match error {
            Disconnected | Unavailable => Self::DeviceNotAvailable,
            NoFreeHandles => Self::StreamIdOverflow,
            InvalidFormat | InvalidRate => Self::StreamConfigNotSupported,
            IllegalArgument => Self::InvalidArgument,
            e => (BackendSpecificError {
                description: e.to_string(),
            })
            .into(),
        }
    }
}
