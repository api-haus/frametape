use frametape::{Annotations, Event, Frame, Manifest};
use wasm_bindgen::prelude::*;

fn error(value: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&value.to_string())
}
fn lines<T: serde::de::DeserializeOwned>(text: &str) -> Result<Vec<T>, JsValue> {
    text.lines()
        .map(|line| serde_json::from_str(line).map_err(error))
        .collect()
}

/// Browser-owned model, using the same validation and confirmed picture mapping as native review.
#[wasm_bindgen]
pub struct Recording {
    manifest: Manifest,
    frames: Vec<Frame>,
    events: Vec<Event>,
    annotations: Annotations,
}

#[wasm_bindgen]
impl Recording {
    #[wasm_bindgen(constructor)]
    pub fn new(
        manifest: &str,
        frames: &str,
        events: &str,
        annotations: &str,
    ) -> Result<Recording, JsValue> {
        let manifest: Manifest = serde_json::from_str(manifest).map_err(error)?;
        frametape::validate_manifest(&manifest).map_err(error)?;
        let mut frames: Vec<Frame> = lines(frames)?;
        frametape::validate_timeline(&manifest, &frames).map_err(error)?;
        let events: Vec<Event> = lines(events)?;
        if events.len() as u64 != manifest.event_count {
            return Err(error("event count differs from manifest"));
        }
        for event in &events {
            frametape::validate_event(&manifest, event).map_err(error)?;
            frametape::apply_video_mapping(&mut frames, event).map_err(error)?;
        }
        let annotations = if annotations.is_empty() {
            Annotations::default()
        } else {
            Annotations::parse(annotations).map_err(error)?
        };
        frametape::validate_annotations(&annotations, manifest.frame_count).map_err(error)?;
        Ok(Self {
            manifest,
            frames,
            events,
            annotations,
        })
    }
    pub fn metadata(&self) -> String {
        serde_json::json!({"manifest":self.manifest,"frames":self.frames,"annotations":self.annotations}).to_string()
    }
    pub fn events_at(&self, index: i32) -> String {
        serde_json::to_string(
            &self
                .events
                .iter()
                .filter(|event| {
                    index < 0
                        || event.frame == Some(index as u64)
                        || event.frame.is_none()
                            && frametape::frame_at(&self.frames, event.timestamp_ns)
                                .is_some_and(|frame| frame.index == index as u64)
                })
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }
    pub fn media_plan(&self, probe: &str) -> Result<String, JsValue> {
        let probe = serde_json::from_str(probe).map_err(error)?;
        let plan = frametape::playback::PlaybackPlan::new(&self.frames, &probe).map_err(error)?;
        serde_json::to_string(&plan).map_err(error)
    }
    pub fn verify_media(&self, plan: &str, probe: &str) -> Result<String, JsValue> {
        let plan: frametape::playback::PlaybackPlan = serde_json::from_str(plan).map_err(error)?;
        let probe = serde_json::from_str(probe).map_err(error)?;
        serde_json::to_string(&plan.verify(&probe).map_err(error)?).map_err(error)
    }
    pub fn annotations(&self, text: &str) -> Result<String, JsValue> {
        let value = Annotations::parse(text).map_err(error)?;
        frametape::validate_annotations(&value, self.manifest.frame_count).map_err(error)?;
        serde_json::to_string(&value).map_err(error)
    }
}
