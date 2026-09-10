use std::time::Duration;

/// A monotonic time instance associated with a stream, retrieved from either:
///
/// 1. A timestamp provided to the stream's underlying audio data callback or
/// 2. The same time source used to generate timestamps for a stream's underlying audio data
///    callback.
///
/// `StreamInstant` represents a moment on a stream's monotonic clock. Because the underlying clock
/// is monotonic, `StreamInstant` values are always positive and increasing.
///
/// Within a single stream, all instants share the same clock, so arithmetic between them is
/// meaningful. Across different streams, origins are not guaranteed to be shared. On some hosts
/// each stream starts its own independent clock at zero, so subtracting a timestamp from one
/// stream and one from another may produce a meaningless result.
///
/// ## Time sources by host
///
/// | Host | Time source |
/// | ---- | ----------- |
/// | AAudio | `AAudioStream_getTimestamp(CLOCK_MONOTONIC)` when valid; otherwise the callback's
///   monotonic instant (inspect [`OutputCallbackInfo::timestamp_source`]) |
/// | ALSA | `snd_pcm_status_get_htstamp()` |
/// | ASIO | `timeGetTime()` |
/// | AudioWorklet | `AudioContext.currentTime` |
/// | CoreAudio | `mach_absolute_time()` |
/// | JACK | `jack_get_time()` |
/// | PipeWire | `pw_stream_get_time_n()` |
/// | PulseAudio | `std::time::Instant` |
/// | WASAPI | `QueryPerformanceCounter()` |
/// | WebAudio | `AudioContext.currentTime` |
///
/// > **Disclaimer:** These system calls might change over time.
///
/// > **Note:** The `+` and `-` operators on `StreamInstant` may panic if the result cannot be
/// > represented as a `StreamInstant`. Use [`checked_add`][StreamInstant::checked_add] or
/// > [`checked_sub`][StreamInstant::checked_sub] for non-panicking variants.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub struct StreamInstant {
    secs: u64,
    nanos: u32,
}

/// A timestamp associated with a call to an input stream's data callback.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub struct InputStreamTimestamp {
    /// The instant the stream's data callback was invoked.
    pub callback: StreamInstant,
    /// The instant that data was captured from the device.
    ///
    /// E.g. The instant data was read from an ADC.
    pub capture: StreamInstant,
}

/// A timestamp associated with a call to an output stream's data callback.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub struct OutputStreamTimestamp {
    /// The instant the stream's data callback was invoked.
    pub callback: StreamInstant,
    /// The predicted instant that data written will be delivered to the device for playback.
    ///
    /// E.g. The instant data will be played by a DAC.
    pub playback: StreamInstant,
}

/// Identifies how an output callback's playback timestamp was obtained.
///
/// This classification is paired with the timestamp for one callback. It does not describe
/// whether the timestamp is valid or monotonic; hosts must handle those conditions separately.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub enum OutputTimestampSource {
    /// The host reported the device presentation time for the callback's first frame.
    DevicePresentation,
    /// The host could not provide a device presentation time and used its monotonic clock.
    MonotonicFallback,
    /// The host does not expose a stable provenance classification.
    Unspecified,
}

/// Why the host could not supply a usable device presentation timestamp.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub enum OutputTimestampFallbackReason {
    Unavailable,
    Unsupported,
    Invalid,
    NonMonotonic,
    ClockDomainMismatch,
}

/// Fixed-size evidence for one host timestamp query. Deltas contain no clock origins.
/// This is diagnostic information, not an additional timestamp validity contract.
#[derive(Copy, Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct OutputTimestampDiagnostics {
    /// Rejection category, absent for accepted device timestamps.
    pub fallback_reason: Option<OutputTimestampFallbackReason>,
    /// Native query error code, absent when the query succeeded.
    pub query_error_code: Option<i32>,
    /// Frame-position change since the previous successful raw query.
    pub anchor_frame_delta: Option<i64>,
    /// Time change since the previous successful raw query, in nanoseconds.
    pub anchor_time_delta_ns: Option<i64>,
    /// Application frame-position change since the previous callback.
    pub app_frame_delta: Option<i64>,
    /// Projected presentation minus this callback's monotonic time, in nanoseconds.
    pub projected_ahead_ns: Option<i64>,
    /// Projected presentation minus the last accepted presentation, in nanoseconds.
    pub projected_step_ns: Option<i64>,
}

#[cfg(any(target_os = "android", test))]
#[derive(Default)]
pub(crate) struct OutputTimestampHistory {
    last_device_presentation: Option<StreamInstant>,
    previous_anchor: Option<(i64, i64)>,
    previous_app_frame: Option<i64>,
}

#[cfg(any(target_os = "android", test))]
impl OutputTimestampHistory {
    pub(crate) fn observe(
        &mut self,
        anchor: Result<(i64, i64), OutputTimestampFallbackReason>,
        app_frame: i64,
        sample_rate: u32,
        fallback: StreamInstant,
        query_error_code: Option<i32>,
    ) -> OutputTimestampMapping {
        let mut mapped = map_output_timestamp(
            anchor,
            app_frame,
            sample_rate,
            true,
            self.last_device_presentation,
            fallback,
        );
        let mut diagnostics = OutputTimestampDiagnostics {
            fallback_reason: mapped.diagnostics.fallback_reason,
            query_error_code,
            app_frame_delta: self
                .previous_app_frame
                .and_then(|previous| app_frame.checked_sub(previous)),
            ..Default::default()
        };
        if let Ok((frame, nanos)) = anchor {
            if let Some((previous_frame, previous_nanos)) = self.previous_anchor {
                diagnostics.anchor_frame_delta = frame.checked_sub(previous_frame);
                diagnostics.anchor_time_delta_ns = nanos.checked_sub(previous_nanos);
            }
            if let Some(projected) =
                stream_instant_from_anchor(frame, nanos, app_frame, sample_rate)
            {
                let delta = |earlier: StreamInstant| {
                    i64::try_from(projected.as_nanos() as i128 - earlier.as_nanos() as i128).ok()
                };
                diagnostics.projected_ahead_ns = delta(fallback);
                diagnostics.projected_step_ns = self.last_device_presentation.and_then(delta);
            }
            // Raw-query history includes rejected projections, so the next event
            // can distinguish anchor motion from the app-frame extrapolation.
            self.previous_anchor = Some((frame, nanos));
        }
        self.previous_app_frame = Some(app_frame);
        mapped.diagnostics = diagnostics;
        if mapped.source == OutputTimestampSource::DevicePresentation {
            self.last_device_presentation = Some(mapped.instant);
        }
        mapped
    }
}

#[cfg(any(target_os = "android", test))]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) struct OutputTimestampMapping {
    instant: StreamInstant,
    source: OutputTimestampSource,
    diagnostics: OutputTimestampDiagnostics,
}

#[cfg(any(target_os = "android", test))]
impl OutputTimestampMapping {
    pub(crate) fn instant(self) -> StreamInstant {
        self.instant
    }

    pub(crate) fn source(self) -> OutputTimestampSource {
        self.source
    }

    pub(crate) fn diagnostics(self) -> OutputTimestampDiagnostics {
        self.diagnostics
    }
}

#[cfg(any(target_os = "android", test))]
pub(crate) fn stream_instant_from_anchor(
    anchor_frame: i64,
    anchor_nanos: i64,
    app_frame: i64,
    sample_rate: u32,
) -> Option<StreamInstant> {
    if anchor_nanos < 0 || sample_rate == 0 {
        return None;
    }
    let frame_delta = (app_frame as i128).checked_sub(anchor_frame as i128)?;
    let offset_nanos = frame_delta
        .checked_mul(1_000_000_000)?
        .checked_div(sample_rate as i128)?;
    let projected_nanos = (anchor_nanos as i128).checked_add(offset_nanos)?;
    let projected_nanos = u64::try_from(projected_nanos).ok()?;
    Some(StreamInstant::from_nanos(projected_nanos))
}

#[cfg(any(target_os = "android", test))]
pub(crate) fn map_output_timestamp(
    anchor: Result<(i64, i64), OutputTimestampFallbackReason>,
    app_frame: i64,
    sample_rate: u32,
    monotonic_domain: bool,
    last_device_presentation: Option<StreamInstant>,
    fallback: StreamInstant,
) -> OutputTimestampMapping {
    let device_presentation = if !monotonic_domain {
        Err(OutputTimestampFallbackReason::ClockDomainMismatch)
    } else {
        anchor
            .and_then(|(anchor_frame, anchor_nanos)| {
                stream_instant_from_anchor(anchor_frame, anchor_nanos, app_frame, sample_rate)
                    .ok_or(OutputTimestampFallbackReason::Invalid)
            })
            .and_then(|instant| {
                if instant < fallback {
                    Err(OutputTimestampFallbackReason::Invalid)
                } else if last_device_presentation.is_some_and(|last| instant <= last) {
                    Err(OutputTimestampFallbackReason::NonMonotonic)
                } else {
                    Ok(instant)
                }
            })
    };

    match device_presentation {
        Ok(instant) => OutputTimestampMapping {
            instant,
            source: OutputTimestampSource::DevicePresentation,
            diagnostics: OutputTimestampDiagnostics::default(),
        },
        Err(fallback_reason) => OutputTimestampMapping {
            instant: fallback,
            source: OutputTimestampSource::MonotonicFallback,
            diagnostics: OutputTimestampDiagnostics {
                fallback_reason: Some(fallback_reason),
                ..Default::default()
            },
        },
    }
}

/// Information relevant to a single call to the user's input stream data callback.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub struct InputCallbackInfo {
    pub(crate) timestamp: InputStreamTimestamp,
}

/// Information relevant to a single call to the user's output stream data callback.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub struct OutputCallbackInfo {
    pub(crate) timestamp: OutputStreamTimestamp,
    pub(crate) timestamp_source: OutputTimestampSource,
    timestamp_diagnostics: Option<OutputTimestampDiagnostics>,
}

impl StreamInstant {
    /// A `StreamInstant` with `secs` and `nanos` both set to zero.
    pub const ZERO: Self = Self { secs: 0, nanos: 0 };

    /// Returns the amount of time elapsed from `earlier` to `self`, or `None` if `earlier` is
    /// later than `self`.
    pub fn checked_duration_since(&self, earlier: StreamInstant) -> Option<Duration> {
        if self < &earlier {
            return None;
        }
        let delta = self.as_nanos() - earlier.as_nanos();
        let secs = u64::try_from(delta / 1_000_000_000).ok()?;
        let subsec_nanos = (delta % 1_000_000_000) as u32;
        Some(Duration::new(secs, subsec_nanos))
    }

    /// Returns the amount of time elapsed from `earlier` to `self`, saturating to
    /// [`Duration::ZERO`] if `earlier` is later than `self`.
    pub fn saturating_duration_since(&self, earlier: StreamInstant) -> Duration {
        self.checked_duration_since(earlier).unwrap_or_default()
    }

    /// Returns the amount of time elapsed from `earlier` to `self`, saturating to
    /// [`Duration::ZERO`] if `earlier` is later than `self`.
    pub fn duration_since(&self, earlier: StreamInstant) -> Duration {
        self.saturating_duration_since(earlier)
    }

    /// Returns `Some(t)` where `t` is `self + duration`, or `None` if the result cannot be
    /// represented as a `StreamInstant`.
    pub fn checked_add(&self, duration: Duration) -> Option<Self> {
        let total = self.as_nanos().checked_add(duration.as_nanos())?;
        let secs = u64::try_from(total / 1_000_000_000).ok()?;
        let nanos = (total % 1_000_000_000) as u32;
        Some(Self { secs, nanos })
    }

    /// Returns `Some(t)` where `t` is `self - duration`, or `None` if the result cannot be
    /// represented as a `StreamInstant` (i.e. would be negative).
    pub fn checked_sub(&self, duration: Duration) -> Option<Self> {
        let total = self.as_nanos().checked_sub(duration.as_nanos())?;
        let secs = u64::try_from(total / 1_000_000_000).ok()?;
        let nanos = (total % 1_000_000_000) as u32;
        Some(Self { secs, nanos })
    }

    /// Returns the total number of nanoseconds contained by this `StreamInstant`.
    pub fn as_nanos(&self) -> u128 {
        self.secs as u128 * 1_000_000_000 + self.nanos as u128
    }

    /// Creates a new `StreamInstant` from the specified number of nanoseconds.
    ///
    /// Note: Using this on the return value of `as_nanos()` might cause unexpected behavior:
    /// `as_nanos()` returns a `u128`, and can return values that do not fit in `u64`, e.g. 585
    /// years. Instead, consider using the pattern
    /// `StreamInstant::new(t.as_secs(), t.subsec_nanos())` if you cannot copy/clone the
    /// `StreamInstant` directly.
    pub fn from_nanos(nanos: u64) -> Self {
        let secs = nanos / 1_000_000_000;
        let subsec_nanos = (nanos % 1_000_000_000) as u32;
        Self::new(secs, subsec_nanos)
    }

    /// Creates a new `StreamInstant` from the specified number of milliseconds.
    pub fn from_millis(millis: u64) -> Self {
        Self::new(millis / 1_000, (millis % 1_000 * 1_000_000) as u32)
    }

    /// Creates a new `StreamInstant` from the specified number of microseconds.
    pub fn from_micros(micros: u64) -> Self {
        Self::new(micros / 1_000_000, (micros % 1_000_000 * 1_000) as u32)
    }

    /// Creates a new `StreamInstant` from the specified number of seconds represented as `f64`.
    ///
    /// # Panics
    ///
    /// Panics if `secs` is negative, not finite, or overflows the range of `StreamInstant`.
    pub fn from_secs_f64(secs: f64) -> Self {
        const NANOS_PER_SEC: u128 = 1_000_000_000;
        const MAX_NANOS: f64 = ((u64::MAX as u128 + 1) * NANOS_PER_SEC) as f64;
        let nanos = secs * NANOS_PER_SEC as f64;
        if !(0.0..MAX_NANOS).contains(&nanos) {
            panic!("StreamInstant::from_secs_f64 called with invalid value: {secs}");
        }
        let nanos = nanos as u128;
        Self::new(
            (nanos / NANOS_PER_SEC) as u64,
            (nanos % NANOS_PER_SEC) as u32,
        )
    }

    /// Creates a new `StreamInstant` from the specified number of whole seconds and additional
    /// nanoseconds.
    ///
    /// If `nanos` is greater than or equal to 1 billion (the number of nanoseconds in a second),
    /// the excess carries over into `secs`.
    ///
    /// # Panics
    ///
    /// Panics if the carry from `nanos` overflows the seconds counter.
    pub fn new(secs: u64, nanos: u32) -> Self {
        let carry = nanos / 1_000_000_000;
        let subsec_nanos = nanos % 1_000_000_000;
        let secs = secs
            .checked_add(carry as u64)
            .expect("overflow in StreamInstant::new");
        Self {
            secs,
            nanos: subsec_nanos,
        }
    }
}

impl std::ops::Add<Duration> for StreamInstant {
    type Output = Self;

    /// # Panics
    ///
    /// Panics if the result overflows the range of `StreamInstant`. Use
    /// [`checked_add`][StreamInstant::checked_add] for a non-panicking variant.
    #[inline]
    fn add(self, rhs: Duration) -> Self::Output {
        self.checked_add(rhs)
            .expect("overflow when adding duration to stream instant")
    }
}

impl std::ops::AddAssign<Duration> for StreamInstant {
    #[inline]
    fn add_assign(&mut self, rhs: Duration) {
        *self = *self + rhs;
    }
}

impl std::ops::Sub<Duration> for StreamInstant {
    type Output = Self;

    /// # Panics
    ///
    /// Panics if the result underflows the range of `StreamInstant`. Use
    /// [`checked_sub`][StreamInstant::checked_sub] for a non-panicking variant.
    #[inline]
    fn sub(self, rhs: Duration) -> Self::Output {
        self.checked_sub(rhs)
            .expect("underflow when subtracting duration from stream instant")
    }
}

impl std::ops::SubAssign<Duration> for StreamInstant {
    #[inline]
    fn sub_assign(&mut self, rhs: Duration) {
        *self = *self - rhs;
    }
}

impl std::ops::Sub<StreamInstant> for StreamInstant {
    type Output = Duration;

    /// Returns the duration from `rhs` to `self`, saturating to [`Duration::ZERO`] if `rhs` is
    /// later than `self`.
    #[inline]
    fn sub(self, rhs: StreamInstant) -> Self::Output {
        self.saturating_duration_since(rhs)
    }
}

impl InputCallbackInfo {
    pub fn new(timestamp: InputStreamTimestamp) -> Self {
        Self { timestamp }
    }

    /// The timestamp associated with the call to an input stream's data callback.
    pub fn timestamp(&self) -> InputStreamTimestamp {
        self.timestamp
    }
}

impl OutputCallbackInfo {
    pub fn new(timestamp: OutputStreamTimestamp) -> Self {
        Self {
            timestamp,
            timestamp_source: OutputTimestampSource::Unspecified,
            timestamp_diagnostics: None,
        }
    }

    #[cfg(any(target_os = "android", test))]
    pub(crate) fn new_with_timestamp_source(
        timestamp: OutputStreamTimestamp,
        timestamp_source: OutputTimestampSource,
    ) -> Self {
        Self {
            timestamp,
            timestamp_source,
            timestamp_diagnostics: None,
        }
    }

    #[cfg(any(target_os = "android", test))]
    pub(crate) fn with_timestamp_diagnostics(
        mut self,
        diagnostics: OutputTimestampDiagnostics,
    ) -> Self {
        self.timestamp_diagnostics = Some(diagnostics);
        self
    }

    /// Host query evidence paired with this callback, if the host provides it.
    pub fn timestamp_diagnostics(&self) -> Option<OutputTimestampDiagnostics> {
        self.timestamp_diagnostics
    }

    /// The timestamp associated with the call to an output stream's data callback.
    pub fn timestamp(&self) -> OutputStreamTimestamp {
        self.timestamp
    }

    /// Returns the provenance of this callback's playback timestamp.
    pub fn timestamp_source(&self) -> OutputTimestampSource {
        self.timestamp_source
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_instant() {
        let z = StreamInstant::ZERO; // origin
        let a = StreamInstant::new(2, 0);
        let max = StreamInstant::new(u64::MAX, 999_999_999); // largest representable instant

        assert_eq!(
            a.checked_sub(Duration::from_secs(1)),
            Some(StreamInstant::new(1, 0))
        );
        assert_eq!(
            a.checked_sub(Duration::from_secs(2)),
            Some(StreamInstant::ZERO)
        );
        assert_eq!(a.checked_sub(Duration::from_secs(3)), None); // would go below zero
        assert_eq!(z.checked_sub(Duration::from_nanos(1)), None); // underflow at origin

        assert_eq!(
            a.checked_add(Duration::from_secs(1)),
            Some(StreamInstant::new(3, 0))
        );
        assert_eq!(max.checked_add(Duration::from_nanos(1)), None); // overflow

        assert_eq!(a.duration_since(z), Duration::from_secs(2));
        assert_eq!(z.duration_since(a), Duration::ZERO); // saturates
        assert_eq!(a.checked_duration_since(z), Some(Duration::from_secs(2)));
        assert_eq!(z.checked_duration_since(a), None);
        assert_eq!(a.saturating_duration_since(z), Duration::from_secs(2));
        assert_eq!(z.saturating_duration_since(a), Duration::ZERO);

        assert_eq!(z + Duration::from_secs(2), a);
        assert_eq!(a - Duration::from_secs(2), z);
        assert_eq!(a - z, Duration::from_secs(2));
        assert_eq!(z - a, Duration::ZERO); // saturates via Sub<StreamInstant>
        let mut c = z;
        c += Duration::from_secs(2);
        assert_eq!(c, a);
        let mut d = a;
        d -= Duration::from_secs(2);
        assert_eq!(d, z);

        // nanosecond carry
        assert_eq!(
            StreamInstant::new(1, 1_500_000_000),
            StreamInstant::new(2, 500_000_000)
        );
        assert_eq!(
            StreamInstant::new(0, 1_000_000_000),
            StreamInstant::new(1, 0)
        );

        // basic round-trip
        assert_eq!(
            StreamInstant::from_secs_f64(1.5),
            StreamInstant::new(1, 500_000_000)
        );
        assert_eq!(StreamInstant::from_secs_f64(0.0), z);
    }

    #[test]
    fn output_timestamp_source_is_paired_with_callback_info() {
        let timestamp = OutputStreamTimestamp {
            callback: StreamInstant::from_millis(10),
            playback: StreamInstant::from_millis(20),
        };

        let unspecified = OutputCallbackInfo::new(timestamp);
        assert_eq!(unspecified.timestamp_diagnostics(), None);
        assert_eq!(
            unspecified.timestamp_source(),
            OutputTimestampSource::Unspecified
        );

        let device = OutputCallbackInfo::new_with_timestamp_source(
            timestamp,
            OutputTimestampSource::DevicePresentation,
        );
        assert_eq!(device.timestamp(), timestamp);
        assert_eq!(
            device.timestamp_source(),
            OutputTimestampSource::DevicePresentation
        );

        let fallback = OutputCallbackInfo::new_with_timestamp_source(
            timestamp,
            OutputTimestampSource::MonotonicFallback,
        );
        assert_eq!(fallback.timestamp(), timestamp);
        assert_eq!(
            fallback.timestamp_source(),
            OutputTimestampSource::MonotonicFallback
        );
    }

    #[test]
    fn output_timestamp_diagnostics_distinguish_projection_rejection_and_query_error() {
        let mut history = OutputTimestampHistory::default();
        let first = history.observe(
            Ok((0, 1_000_000_000)),
            8_000,
            48_000,
            StreamInstant::from_millis(1_000),
            None,
        );
        assert_eq!(first.source(), OutputTimestampSource::DevicePresentation);
        // Both raw anchor coordinates advance, but the projected app frame regresses.
        let rejected = history.observe(
            Ok((960, 1_009_000_000)),
            8_480,
            48_000,
            StreamInstant::from_millis(1_010),
            None,
        );
        assert_eq!(rejected.source(), OutputTimestampSource::MonotonicFallback);
        assert_eq!(
            rejected.diagnostics.fallback_reason,
            Some(OutputTimestampFallbackReason::NonMonotonic)
        );
        assert_eq!(rejected.diagnostics.anchor_frame_delta, Some(960));
        assert_eq!(rejected.diagnostics.anchor_time_delta_ns, Some(9_000_000));
        assert_eq!(rejected.diagnostics.app_frame_delta, Some(480));
        assert_eq!(rejected.diagnostics.projected_step_ns, Some(-1_000_000));
        assert_eq!(rejected.diagnostics.projected_ahead_ns, Some(155_666_666));
        assert_eq!(rejected.diagnostics.query_error_code, None);
        let info = OutputCallbackInfo::new_with_timestamp_source(
            OutputStreamTimestamp {
                callback: StreamInstant::from_millis(1_010),
                playback: rejected.instant(),
            },
            rejected.source(),
        )
        .with_timestamp_diagnostics(rejected.diagnostics());
        assert_eq!(info.timestamp_diagnostics(), Some(rejected.diagnostics()));
        assert_eq!(
            info.timestamp_source(),
            OutputTimestampSource::MonotonicFallback
        );
        let missing = history.observe(
            Err(OutputTimestampFallbackReason::Unavailable),
            8_960,
            48_000,
            StreamInstant::from_millis(1_020),
            Some(-899),
        );
        assert_eq!(missing.diagnostics.query_error_code, Some(-899));
        assert_eq!(
            missing.diagnostics.fallback_reason,
            Some(OutputTimestampFallbackReason::Unavailable)
        );
        assert_eq!(missing.diagnostics.anchor_frame_delta, None);
        assert_eq!(missing.diagnostics.projected_step_ns, None);
        let recovered = history.observe(
            Ok((1_920, 1_030_000_000)),
            9_440,
            48_000,
            StreamInstant::from_millis(1_030),
            None,
        );
        assert_eq!(
            recovered.source(),
            OutputTimestampSource::DevicePresentation
        );
        assert_eq!(recovered.diagnostics.fallback_reason, None);
        assert_eq!(recovered.diagnostics.query_error_code, None);
        assert_eq!(recovered.diagnostics.anchor_frame_delta, Some(960));
        assert_eq!(recovered.diagnostics.app_frame_delta, Some(480));
        assert_eq!(recovered.diagnostics.projected_step_ns, Some(20_000_000));
    }

    #[test]
    fn output_timestamp_mapping_classifies_device_and_fallback_outcomes() {
        let fallback = StreamInstant::from_millis(99);
        let device =
            map_output_timestamp(Ok((100, 1_000_000_000)), 148, 48_000, true, None, fallback);
        assert_eq!(device.instant(), StreamInstant::from_millis(1_001));
        assert_eq!(device.source(), OutputTimestampSource::DevicePresentation);
        assert_eq!(device.diagnostics.fallback_reason, None);

        for (anchor, monotonic_domain, last, reason) in [
            (
                Err(OutputTimestampFallbackReason::Unavailable),
                true,
                None,
                OutputTimestampFallbackReason::Unavailable,
            ),
            (
                Err(OutputTimestampFallbackReason::Unsupported),
                true,
                None,
                OutputTimestampFallbackReason::Unsupported,
            ),
            (
                Ok((0, -1)),
                true,
                None,
                OutputTimestampFallbackReason::Invalid,
            ),
            (
                Ok((0, 50_000_000)),
                true,
                Some(StreamInstant::from_millis(40)),
                OutputTimestampFallbackReason::Invalid,
            ),
            (
                Ok((100, 1_000_000_000)),
                true,
                Some(StreamInstant::from_millis(1_001)),
                OutputTimestampFallbackReason::NonMonotonic,
            ),
            (
                Ok((100, 1_000_000_000)),
                false,
                None,
                OutputTimestampFallbackReason::ClockDomainMismatch,
            ),
        ] {
            let mapped =
                map_output_timestamp(anchor, 148, 48_000, monotonic_domain, last, fallback);
            assert_eq!(mapped.instant(), fallback);
            assert_eq!(mapped.source(), OutputTimestampSource::MonotonicFallback);
            assert_eq!(mapped.diagnostics.fallback_reason, Some(reason));
        }
    }

    #[test]
    #[should_panic]
    fn test_stream_instant_new_overflow() {
        StreamInstant::new(u64::MAX, 1_000_000_000); // carry overflows u64
    }

    #[test]
    #[should_panic]
    fn test_stream_instant_from_secs_f64_negative() {
        StreamInstant::from_secs_f64(-1.0);
    }

    #[test]
    #[should_panic]
    fn test_stream_instant_from_secs_f64_nan() {
        StreamInstant::from_secs_f64(f64::NAN);
    }

    #[test]
    #[should_panic]
    fn test_stream_instant_from_secs_f64_infinite() {
        StreamInstant::from_secs_f64(f64::INFINITY);
    }
}
