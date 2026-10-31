//! Agent execution, editing tools, and validated profiles, independent of the chat UI.
use crate::providers::ActiveModel;
use aigle_core::EditParams;
use aigle_doc::{ParamOp, PhotoDoc};
use aigle_engine_api::{EncodedFormat, OffscreenOpts, Session};
use anyhow::{Context as _, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures::channel::mpsc::UnboundedSender;
use rig_core as rig;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Deserialize, Serialize)]
pub(crate) struct Profile {
    pub(crate) explanation: String,
    pub(crate) adjustments: BTreeMap<String, Value>,
}

fn profile(base: &EditParams, value: &str) -> Result<(EditParams, String)> {
    let p: Profile = serde_json::from_str(value)?;
    ensure!(!p.adjustments.is_empty(), "The profile has no adjustments");
    let mut ops = Vec::new();
    for (path, value) in p.adjustments {
        ensure!(
            path != "process_version",
            "Process version is not an editing control"
        );
        if let Some(spec) = base.spec(&path) {
            let number = value.as_f64().context("Expected a numeric adjustment")?;
            ensure!(
                number.is_finite() && number >= spec.min as f64 && number <= spec.max as f64,
                "Out-of-range adjustment: {path}"
            );
        }
        ops.push(ParamOp::Set {
            path: path.as_str().into(),
            value,
        });
    }
    // Use the same typed path validation and normalization as interactive edits.
    let params = PhotoDoc::new(base.clone())?.preview(&ops)?;
    Ok((params, p.explanation))
}
/// Keep structured edits intact when committing the visually inspected candidate.
pub(crate) fn profile_ops(original: &EditParams, edited: &EditParams) -> Vec<ParamOp> {
    let before = serde_json::to_value(original).expect("serializable edit parameters");
    let after = serde_json::to_value(edited).expect("serializable edit parameters");
    after
        .as_object()
        .expect("edit parameter object")
        .iter()
        .filter(|(path, value)| before.get(*path) != Some(*value))
        .map(|(path, value)| ParamOp::Set {
            path: path.as_str().into(),
            value: value.clone(),
        })
        .collect()
}

/// Render through the same engine as the editor, returning typed vision content to Rig.
async fn preview_image(session: &Session, params: &EditParams) -> Result<rig::message::Image> {
    let image = session
        .render_offscreen(
            Arc::new(params.clone()),
            OffscreenOpts {
                format: EncodedFormat::Jpeg { quality: 85 },
                ..Default::default()
            },
        )
        .await
        .map_err(|e| anyhow::anyhow!("Preview failed: {e}"))?;
    Ok(rig::message::Image {
        data: rig::message::DocumentSourceKind::Base64(STANDARD.encode(&image.bytes)),
        media_type: Some(rig::message::ImageMediaType::JPEG),
        ..Default::default()
    })
}

#[derive(Clone)]
struct EditingTools {
    base: Arc<EditParams>,
    session: Session,
    state: Arc<std::sync::Mutex<ProfileState>>,
    cancel: Arc<AtomicBool>,
    events: UnboundedSender<AgentEvent>,
}
struct ProfileState {
    candidate: EditParams,
    proposed: bool,
    inspected: bool,
}
impl EditingTools {
    fn check_cancelled(&self) -> Result<()> {
        ensure!(!self.cancel.load(Ordering::Relaxed), "Cancelled");
        Ok(())
    }
    fn present(&self, args: Profile) -> Result<String> {
        self.check_cancelled()?;
        let (candidate, explanation) = profile(&self.base, &serde_json::to_string(&args)?)?;
        let _ = self.events.unbounded_send(AgentEvent::Profile(args));
        let mut state = self.state.lock().unwrap();
        state.candidate = candidate;
        state.proposed = true;
        state.inspected = false;
        Ok(format!(
            "Profile presented: {explanation}. Call see_preview before finishing."
        ))
    }
    fn finish(&self) -> Result<EditParams> {
        self.check_cancelled()?;
        let state = self.state.lock().unwrap();
        ensure!(
            state.proposed && state.inspected,
            "Model finished without proposing and visually checking an edit. Use a vision model with tool calling."
        );
        Ok(state.candidate.clone())
    }
}
struct PresentProfile(EditingTools);
impl rig::tool::PortableTool for PresentProfile {
    const NAME: &'static str = "present_profile";
    type Args = Profile;
    type Output = String;
    type Error = std::io::Error;
    fn description(&self) -> String {
        "Present photo edits for rendering. Keys are dotted EditParams paths; values may be numbers, booleans, strings, objects, arrays, or null as specified in the editing schema. Values replace the selected path in the starting profile. Include all desired changes on every proposal; omitted paths retain their starting values. Replace arrays to add, change, or remove curve points, masks, spots, or red-eye corrections.".into()
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"explanation":{"type":"string"},"adjustments":{"type":"object","additionalProperties":{}}},"required":["explanation","adjustments"]})
    }
    async fn call(&self, args: Profile) -> std::io::Result<String> {
        self.0.present(args).map_err(std::io::Error::other)
    }
}
/// Image-derived values used by the same automatic controls in the editor.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum SuggestionKind {
    AutoTone,
    AutoWhiteBalance,
    PickWhiteBalance,
    MatchLens,
    SpotSource,
    SampleColor,
}
#[derive(Deserialize)]
struct SuggestionArgs {
    action: SuggestionKind,
    x: Option<f32>,
    y: Option<f32>,
    radius: Option<f32>,
}
struct SuggestEdits(EditingTools);
impl rig::tool::PortableTool for SuggestEdits {
    const NAME: &'static str = "suggest_edits";
    type Args = SuggestionArgs;
    type Output = Value;
    type Error = std::io::Error;
    fn description(&self) -> String {
        "Run the editor's automatic image analysis helpers. Returns suggested adjustments or a healing source coordinate; include desired results in present_profile and inspect with see_preview. Coordinates and radius are normalized to the source image. Does not modify the proposed profile.".into()
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{
            "action":{"type":"string","enum":["auto_tone","auto_white_balance","pick_white_balance","match_lens","spot_source","sample_color"]},
            "x":{"type":"number","minimum":0,"maximum":1},
            "y":{"type":"number","minimum":0,"maximum":1},
            "radius":{"type":"number","exclusiveMinimum":0,"maximum":0.25}
        },"required":["action"]})
    }
    async fn call(&self, args: SuggestionArgs) -> std::io::Result<Value> {
        let result: Result<Value> = async {
            self.0.check_cancelled()?;
            let point = || -> Result<aigle_core::masks::Pt> {
                let x = args.x.context("x is required")?;
                let y = args.y.context("y is required")?;
                ensure!(x.is_finite() && y.is_finite() && (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y), "Coordinates must be within 0..1");
                Ok(aigle_core::masks::Pt { x, y })
            };
            let radius = args.radius.unwrap_or(0.01);
            ensure!(radius.is_finite() && radius > 0.0 && radius <= 0.25, "Radius must be within 0..0.25");
            let session = &self.0.session;
            let suggestion = match args.action {
                SuggestionKind::AutoWhiteBalance | SuggestionKind::PickWhiteBalance => {
                    let picked = matches!(args.action, SuggestionKind::PickWhiteBalance);
                    let wb = if picked {
                        let p = point()?;
                        session.neutral_white_balance(aigle_core::geometry::ImagePoint { x: p.x as f64, y: p.y as f64 }, radius).await?
                    } else { session.auto_white_balance(None).await? };
                    json!({"adjustments":{"wb.temperature":wb.temperature,"wb.tint":wb.tint,"wb.mode":if picked {"custom"} else {"auto"}},"warning":wb.low_confidence_reason})
                }
                SuggestionKind::MatchLens => {
                    let found = session.match_lens().await?;
                    let lens = found.first().context("No matching lens profile")?;
                    json!({"adjustments":{"lens.profile":aigle_core::params::LensProfile::Resolved(lens.data_ref.clone())}})
                }
                SuggestionKind::SampleColor => {
                    let p = point()?;
                    let sample = session.sample(aigle_core::geometry::ImagePoint {x:p.x as f64,y:p.y as f64}, radius, aigle_engine_api::SampleStage::SceneLinear).await?;
                    json!({"rgb":sample.rgb})
                }
                SuggestionKind::SpotSource => json!({"source":session.spot_source(point()?, radius).await?}),
                SuggestionKind::AutoTone => {
                    let analysis = session.analyze(Arc::new(crate::auto_tone::neutral(&self.0.base)), aigle_engine_api::AnalysisSet {
                        histogram:true, clipping:true, wb:false, noise:false, exact:false, output:aigle_engine_api::DisplayProfileId::Srgb,
                    }).await?;
                    let hist = analysis.histogram.context("Histogram unavailable")?;
                    let clip = analysis.clipping.context("Clipping analysis unavailable")?;
                    let adjustments: BTreeMap<String, Value> = crate::auto_tone::ops(&hist, &clip).into_iter().filter_map(|op| match op {
                        ParamOp::Set { path, value } => Some((path.to_string(), value)), _ => None,
                    }).collect();
                    json!({"adjustments":adjustments})
                }
            };
            self.0.check_cancelled()?;
            Ok(suggestion)
        }.await;
        result.map_err(std::io::Error::other)
    }
}

#[derive(Deserialize)]
struct PreviewArgs {}
struct SeePreview(EditingTools);
impl rig::tool::PortableTool for SeePreview {
    const NAME: &'static str = "see_preview";
    type Args = PreviewArgs;
    type Output = rig::tool::ToolOutput;
    type Error = std::io::Error;
    fn description(&self) -> String {
        "Render and visually inspect the latest editing profile.".into()
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{}})
    }
    async fn call(&self, _: PreviewArgs) -> std::io::Result<Self::Output> {
        let result: Result<Self::Output> = async {
            self.0.check_cancelled()?;
            let candidate = {
                let state = self.0.state.lock().unwrap();
                ensure!(state.proposed, "Present a valid profile first");
                state.candidate.clone()
            };
            let image = preview_image(&self.0.session, &candidate).await?;
            self.0.check_cancelled()?;
            if let rig::message::DocumentSourceKind::Base64(data) = &image.data {
                let bytes = STANDARD.decode(data)?;
                let _ = self.0.events.unbounded_send(AgentEvent::Preview(bytes));
            }
            let mut state = self.0.state.lock().unwrap();
            // Tools execute sequentially in the Rig runner below.
            state.inspected = true;
            Ok(rig::tool::ToolOutput::content(vec![
                rig::message::ToolResultContent::text("Rendered preview of the current profile. Inspect it against the user's request; refine or finish."),
                rig::message::ToolResultContent::Image(image),
            ])?)
        }.await;
        result.map_err(std::io::Error::other)
    }
}

pub(crate) fn endpoint_base(endpoint: &str) -> Result<String> {
    let mut url = rig_reqwest::reqwest::Url::parse(endpoint.trim())?;
    ensure!(
        matches!(url.scheme(), "https" | "http") && url.host_str().is_some(),
        "Endpoint must use HTTP or HTTPS"
    );
    // Accept the full URL saved by the original panel as well as a provider base URL.
    let path = url.path().trim_end_matches('/');
    let path = path
        .strip_suffix("/chat/completions")
        .unwrap_or(path)
        .to_string();
    url.set_path(&path);
    ensure!(
        url.query().is_none() && url.fragment().is_none(),
        "Use an endpoint without query parameters or fragments"
    );
    Ok(url.as_str().trim_end_matches('/').to_string())
}

const MAX_TURNS: usize = 100;

#[derive(Clone, Default)]
pub(crate) struct RunStats {
    pub(crate) usage: rig::completion::Usage,
    pub(crate) requests: usize,
    pub(crate) elapsed_seconds: f64,
    // A rate is meaningful only when every response reports both counters.
    cache_complete: bool,
}
impl RunStats {
    fn record(&mut self, usage: rig::completion::Usage) {
        self.cache_complete = (self.requests == 0 || self.cache_complete)
            && usage.input_tokens.is_some()
            && usage.cached_input_tokens.is_some();
        self.requests += 1;
        self.usage += usage;
    }
    pub(crate) fn cache_rate(&self) -> Option<f64> {
        let input = self.usage.input_tokens?;
        (self.cache_complete && input > 0)
            .then(|| self.usage.cached_input_tokens.unwrap_or(0) as f64 / input as f64 * 100.0)
    }
}
struct UsageObserver(Arc<std::sync::Mutex<RunStats>>);
impl rig_agent::agent::AgentHook for UsageObserver {
    async fn on_outcome(
        &self,
        _: &rig_agent::agent::HookContext,
        event: rig_agent::agent::OutcomeEvent<'_>,
    ) -> rig_agent::agent::OutcomeAction {
        if let Some(response) = event.completion() {
            self.0.lock().unwrap().record(response.usage);
        }
        rig_agent::agent::OutcomeAction::proceed()
    }
}

pub(crate) fn run(
    settings: ActiveModel,
    key: String,
    session: Session,
    base: EditParams,
    conversation: String,
    cancel: Arc<AtomicBool>,
    events: UnboundedSender<AgentEvent>,
) -> Result<(EditParams, String)> {
    let base_url = endpoint_base(&settings.endpoint)?;
    ensure!(!settings.model.trim().is_empty(), "Choose a vision model");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let http = rig_reqwest::reqwest::Client::builder().timeout(Duration::from_secs(120)).build()?;
        let mut config = rig::providers::openai::OpenAIConfig::new(key);
        config.base_url = base_url;
        let provider = config.connect(rig_reqwest::ReqwestClient::from(http));
        let specs: Vec<_> = EditParams::scalar_specs().into_iter()
            .map(|(p,s)| json!({"path":p,"min":s.min,"max":s.max,"description":s.doc,"step":s.step})).collect();
        let tools = EditingTools {
            base: Arc::new(base.clone()), session,
            state: Arc::new(std::sync::Mutex::new(ProfileState { candidate: base.clone(), proposed: false, inspected: false })),
            cancel: cancel.clone(), events,
        };
        let stats = Arc::new(std::sync::Mutex::new(RunStats::default()));
        let started = std::time::Instant::now();
        let agent = rig_agent::AgentBuilder::new(provider.chat(settings.model.trim()))
            .preamble(format!("You edit photographs using the full non-destructive editing controls available in the editor. You MUST inspect the supplied image. Use present_profile to propose absolute parameter values relative to the starting profile, then see_preview to inspect the actual rendered result. Refine until it matches the user's intent. Only finish after inspecting a preview of the latest profile. Explain the result briefly. Do not claim to perform pixel generation or unsupported edits. Scalar ranges: {}. Full editing schema: {}. Use dotted paths for fields and array indices. Point curves are arrays of normalized x/y points at curve.point.rgb/red/green/blue. Masks, spots, and red_eye can be replaced with typed arrays, including empty arrays to remove them. Boolean toggles and enum selectors accept their schema types. Coordinates are normalized image coordinates. Use suggest_edits for automatic tone, white balance, lens matching, healing source selection, and scene-linear color sampling for range masks. Reuse existing asset references; do not invent asset identifiers. Preserve process_version. Starting profile: {}", json!(specs), EditParams::json_schema(), serde_json::to_string(&base)?))
            .tool(PresentProfile(tools.clone()))
            .tool(SeePreview(tools.clone()))
            .tool(SuggestEdits(tools.clone()))
            .add_hook(crate::agent_preview::UserImagePreviews)
            .add_hook(UsageObserver(stats.clone()))
            .build();
        let operation = async {
            tools.check_cancelled()?;
            let image = preview_image(&tools.session, &base).await?;
            let prompt = rig::message::Message::User { content: vec![
                rig::message::UserContent::text(conversation),
                rig::message::UserContent::Image(image),
            ] };
            let response = agent.prompt(prompt).max_turns(MAX_TURNS).tool_concurrency(1).run().await?;
            Ok((tools.finish()?, response.output().to_string()))
        };
        let result = tokio::select! {
            result = operation => result,
            _ = async {
                while !cancel.load(Ordering::Relaxed) { tokio::time::sleep(Duration::from_millis(50)).await; }
            } => Err(anyhow::anyhow!("Cancelled")),
        };
        let mut stats = stats.lock().unwrap().clone();
        stats.elapsed_seconds = started.elapsed().as_secs_f64();
        let _ = tools.events.unbounded_send(AgentEvent::Stats(stats));
        result
    })
}

pub(crate) enum AgentEvent {
    Profile(Profile),
    Preview(Vec<u8>),
    Stats(RunStats),
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn run_statistics_accumulate_and_measure_cache_hits() {
        let mut stats = RunStats::default();
        assert_eq!(stats.cache_rate(), None);
        stats.record(rig::completion::Usage {
            input_tokens: Some(100),
            output_tokens: Some(20),
            total_tokens: Some(120),
            cached_input_tokens: Some(80),
            ..Default::default()
        });
        stats.record(rig::completion::Usage {
            input_tokens: Some(300),
            output_tokens: Some(30),
            total_tokens: Some(330),
            cached_input_tokens: Some(120),
            reasoning_tokens: Some(10),
            ..Default::default()
        });
        assert_eq!(stats.requests, 2);
        assert_eq!(stats.usage.total_tokens, Some(450));
        assert_eq!(stats.usage.output_tokens, Some(50));
        assert_eq!(stats.cache_rate(), Some(50.0));
        // Missing cache metrics must not be presented as a zero hit rate.
        stats.record(rig::completion::Usage {
            input_tokens: Some(100),
            ..Default::default()
        });
        assert_eq!(stats.cache_rate(), None);
        assert_eq!(RunStats::default().usage.total_tokens, None);
    }

    #[test]
    fn zero_input_usage_does_not_produce_an_invalid_rate() {
        let mut stats = RunStats::default();
        stats.record(rig::completion::Usage {
            input_tokens: Some(0),
            cached_input_tokens: Some(0),
            ..Default::default()
        });
        assert_eq!(stats.cache_rate(), None);
    }

    #[test]
    fn endpoints_accept_base_and_legacy_full_urls() {
        assert_eq!(
            endpoint_base("https://example.com/v1/").unwrap(),
            "https://example.com/v1"
        );
        assert_eq!(
            endpoint_base("http://localhost:1234/v1/chat/completions").unwrap(),
            "http://localhost:1234/v1"
        );
        assert!(endpoint_base("file:///tmp/model").is_err());
        assert!(endpoint_base("https://example.com/v1?secret=x").is_err());
    }
    #[test]
    fn structured_profiles_survive_preview_and_commit() {
        let base = EditParams::default();
        let proposal = json!({"explanation": "Structured edits", "adjustments": {
            "curve.point.rgb": [{"x":0.0,"y":0.0},{"x":0.5,"y":0.6},{"x":1.0,"y":1.0}],
            "curve.point.red": [{"x":0.0,"y":0.1},{"x":1.0,"y":1.0}],
            "spots": [{"mode":"clone","target":{"x":0.4,"y":0.4},"source":{"x":0.3,"y":0.3},"radius":0.02}],
            "red_eye": [{"center":{"x":0.5,"y":0.5},"radius":0.01}],
            "bw.monochrome": true,
            "wb.mode": "custom",
            "geometry.crop": {"x":0.1,"y":0.1,"w":0.8,"h":0.8},
            "masks": [{"id":1,"name":"Subject","shape":{"kind":"radial","center":{"x":0.5,"y":0.5},"radii":[0.2,0.3],"angle":0.0,"feather":50.0},"opacity":0.8,"adjust":{"exposure":0.5}}]
        }});
        let (candidate, _) = profile(&base, &proposal.to_string()).unwrap();
        assert_eq!(candidate.curve.point.rgb.len(), 3);
        assert_eq!(candidate.curve.point.red.len(), 2);
        assert_eq!(candidate.masks.len(), 1);
        assert_eq!(candidate.spots.len(), 1);
        assert_eq!(candidate.red_eye.len(), 1);
        let committed = PhotoDoc::new(base.clone())
            .unwrap()
            .preview(&profile_ops(&base, &candidate))
            .unwrap();
        assert_eq!(committed, candidate);
        assert_eq!(committed.light, base.light);
        let (cleared, _) = profile(
            &candidate,
            &json!({"explanation":"Clear", "adjustments":{"masks":[],"curve.point.rgb":[]}})
                .to_string(),
        )
        .unwrap();
        assert!(cleared.masks.is_empty());
        assert!(cleared.curve.point.rgb.is_empty());
        assert_eq!(cleared.curve.point.red, candidate.curve.point.red);
    }

    #[test]
    fn structured_profiles_reject_invalid_types_and_paths() {
        let base = EditParams::default();
        for adjustments in [
            json!({"wb.mode":"invalid"}),
            json!({"curve.point.rgb":"invalid"}),
            json!({"bw.monochrome":1}),
            json!({"geometry.typo":true}),
            json!({"masks":[{"id":1}]}),
            json!({"process_version":99}),
        ] {
            assert!(
                profile(
                    &base,
                    &json!({"explanation":"Invalid", "adjustments":adjustments}).to_string()
                )
                .is_err()
            );
        }
    }

    #[test]
    fn profiles_are_validated_and_based_on_starting_edits() {
        let base = EditParams::default();
        let (p, _) = profile(
            &base,
            r#"{"explanation":"Brighter","adjustments":{"light.exposure":1.0}}"#,
        )
        .unwrap();
        assert_eq!(p.get("light.exposure"), Some(1.0));
        assert_eq!(base.get("light.exposure"), Some(0.0));
        assert!(
            profile(
                &base,
                r#"{"explanation":"bad","adjustments":{"light.exposure":1000}}"#
            )
            .is_err()
        );
        assert!(profile(&base, r#"{"explanation":"bad","adjustments":{"bogus":1}}"#).is_err());
    }
}
