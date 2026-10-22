//! Optional software-frame profiling. Submission is not physical display scanout.
use gpui::Window;
use serde_json::{Value, json};
#[cfg(feature = "ui-profiler")]
use std::{cell::RefCell, collections::VecDeque, rc::Rc, time::Instant};

#[cfg(feature = "ui-profiler")]
struct Baseline {
    frame: gpui::profiler::FrameDurationSnapshot,
    input: gpui::profiler::InputLatencySnapshot,
}
#[cfg(feature = "ui-profiler")]
#[derive(Default)]
struct GridPaint {
    pending: Option<Instant>,
    samples_ms: VecDeque<f64>,
    coalesced_installs: u64,
}
#[derive(Clone, Default)]
pub(super) struct PaintProbe {
    #[cfg(feature = "ui-profiler")]
    state: Rc<RefCell<GridPaint>>,
}
impl PaintProbe {
    pub(super) fn painted(&self) {
        #[cfg(feature = "ui-profiler")]
        {
            let mut state = self.state.borrow_mut();
            if let Some(start) = state.pending.take() {
                if state.samples_ms.len() == 1024 {
                    state.samples_ms.pop_front();
                }
                state
                    .samples_ms
                    .push_back(start.elapsed().as_secs_f64() * 1000.);
            }
        }
    }
}
#[derive(Default)]
pub(super) struct UiProfiler {
    #[cfg(feature = "ui-profiler")]
    baseline: Option<Baseline>,
    paint: PaintProbe,
}
impl UiProfiler {
    pub(super) fn paint_probe(&self) -> PaintProbe {
        self.paint.clone()
    }
    pub(super) fn grid_installed(&self) {
        #[cfg(feature = "ui-profiler")]
        {
            let mut state = self.paint.state.borrow_mut();
            if state.pending.replace(Instant::now()).is_some() {
                state.coalesced_installs += 1;
            }
        }
    }
    pub(super) fn reset(&mut self, _window: &Window) {
        #[cfg(feature = "ui-profiler")]
        {
            self.baseline = Some(Baseline {
                frame: _window.frame_duration_snapshot(),
                input: _window.input_latency_snapshot(),
            });
            *self.paint.state.borrow_mut() = GridPaint::default();
        }
    }
    pub(super) fn report(&self, _window: &Window) -> Value {
        #[cfg(feature = "ui-profiler")]
        {
            macro_rules! histogram {
                ($current:expr,$baseline:expr) => {{
                    let mut histogram=$current;
                    if let Some(baseline)=$baseline {
                        if histogram.subtract(baseline).is_err(){return json!({"enabled":true,"error":"histogram baseline mismatch; reset metrics"});}
                    }
                    if histogram.is_empty(){json!({"count":0})} else {
                        json!({"count":histogram.len(),"p50_ms":histogram.value_at_quantile(0.5) as f64/1e6,"p95_ms":histogram.value_at_quantile(0.95) as f64/1e6,"max_ms":histogram.max() as f64/1e6,
                            "recorded_buckets":histogram.iter_recorded().map(|v|json!({"upper_ns":v.value_iterated_to(),"count":v.count_at_value()})).collect::<Vec<_>>()})
                    }
                }};
            }
            let frame = _window.frame_duration_snapshot();
            let input = _window.input_latency_snapshot();
            let draw = histogram!(
                frame.draw_duration_histogram,
                self.baseline
                    .as_ref()
                    .map(|b| &b.frame.draw_duration_histogram)
            );
            let dirty = histogram!(
                frame.dirty_to_present_histogram,
                self.baseline
                    .as_ref()
                    .map(|b| &b.frame.dirty_to_present_histogram)
            );
            let interval = histogram!(
                frame.present_interval_histogram,
                self.baseline
                    .as_ref()
                    .map(|b| &b.frame.present_interval_histogram)
            );
            let latency = histogram!(
                input.latency_histogram,
                self.baseline.as_ref().map(|b| &b.input.latency_histogram)
            );
            let mut event_counts = input.events_per_frame_histogram;
            if let Some(baseline) = &self.baseline {
                let _ = event_counts.subtract(&baseline.input.events_per_frame_histogram);
            }
            let paint = self.paint.state.borrow();
            json!({"enabled":true,"since_reset":self.baseline.is_some(),"grid_preview_mode":if raw_grid_enabled(){"raw_baseline"}else{"bounded_256_bytes"},
                "input_to_platform_submission":latency,"dirty_to_platform_submission":dirty,"draw_duration":draw,"animation_submission_interval":interval,
                "coalesced_events_per_input_frame_p95":event_counts.value_at_quantile(0.95),
                "mid_draw_events_excluded":input.mid_draw_events_dropped.saturating_sub(self.baseline.as_ref().map_or(0,|b|b.input.mid_draw_events_dropped)),
                "grid_install_to_root_paint_ms":paint.samples_ms,"coalesced_grid_installs":paint.coalesced_installs,
                "limits":"GPUI profiler build overhead included. Input may coalesce and includes keyboard/mouse/scroll; mid-draw events excluded. Platform draw submission does not prove GPU completion/physical display. Grid marker is root content paint, not query start or wire first row; last1024 samples."})
        }
        #[cfg(not(feature = "ui-profiler"))]
        json!({"enabled":false,"enable_with":"cargo build --release -p dbide-ui-spike --features ui-profiler"})
    }
}
/// The legacy unbounded presentation path exists only in opt-in profiler builds
/// for same-binary A/B measurements; normal builds always use bounded previews.
pub(super) fn raw_grid_enabled() -> bool {
    cfg!(feature = "ui-profiler")
        && std::env::var_os("DBIDE_UI_RAW_GRID").as_deref() == Some(std::ffi::OsStr::new("1"))
}
