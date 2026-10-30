use std::ffi::{c_char, c_void, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use merman::svg::{
    HostMeasurementResult, HostTextMeasurement, HostTextMeasurementRequest, HostTextMeasurer,
    MeasurementProfileId, TextMeasurementOperation as Op, TextMeasurementPhase,
    TextMeasurementPolicy, TextMeasurementProfileIdentity, TextMetrics, WrapMode,
};
use merman::{OperationControl, RenderOutput, RenderRequest, Renderer, SvgEnvironment, SvgRequest};

#[repr(C)]
pub struct MeasureRequest {
    text: *const c_char,
    text_len: usize,
    font_family: *const c_char,
    font_family_len: usize,
    font_size: f64,
    bold: i32,
    italic: i32,
    max_width: f64,
    html_like: i32,
    kind: i32,
}

#[repr(C)]
#[derive(Default)]
pub struct MeasureResult {
    width: f64,
    height: f64,
    line_count: u32,
    length: f64,
    left: f64,
    right: f64,
    raw_width: f64,
}

type MeasureFn = extern "C" fn(*const MeasureRequest, *mut MeasureResult, *mut c_void) -> i32;

const METRICS: i32 = 0;
const WIDTH: i32 = 1;
const HEIGHT: i32 = 2;
const EXTENTS: i32 = 3;
const WRAPPED_RAW: i32 = 4;

struct HostMeasurer {
    measure: MeasureFn,
    context: usize,
}

impl HostTextMeasurer for HostMeasurer {
    fn measure(&self, request: HostTextMeasurementRequest<'_>) -> HostMeasurementResult {
        let kind = match request.operation {
            Op::Measure | Op::Wrapped | Op::MermaidCalculateTextDimensions => METRICS,
            Op::WrappedWithRawWidth => WRAPPED_RAW,
            Op::ComputedLength
            | Op::SimpleBBoxWidth
            | Op::RawBBoxWidth
            | Op::TspanBBoxWidth
            | Op::WrapProbeBBoxWidth
            | Op::BoundingClientRectWidth
            | Op::CanvasMeasureTextWidth => WIDTH,
            Op::TspanBBoxHeight | Op::SimpleBBoxHeight | Op::RawBBoxHeight => HEIGHT,
            Op::BBoxX | Op::BBoxXWithAsciiOverhang | Op::TitleBBoxX => EXTENTS,
            Op::CreateTextBBoxYOffset | Op::CreateTextMiddleBBoxYOffset => return Ok(None),
        };
        let style = request.style;
        let family = style.font_family.as_deref().unwrap_or("");
        let weight = style.font_weight.as_deref().unwrap_or("");
        let bold = weight == "bold" || weight == "bolder" || weight.parse::<u32>().is_ok_and(|w| w >= 600);
        let c_request = MeasureRequest {
            text: request.text.as_ptr().cast(),
            text_len: request.text.len(),
            font_family: family.as_ptr().cast(),
            font_family_len: family.len(),
            font_size: style.font_size,
            bold: bold as i32,
            italic: matches!(style.font_style.as_deref(), Some("italic" | "oblique")) as i32,
            max_width: request.max_width.unwrap_or(-1.0),
            html_like: matches!(request.wrap_mode, WrapMode::HtmlLike) as i32,
            kind,
        };
        let mut out = MeasureResult::default();
        if (self.measure)(&c_request, &mut out, self.context as *mut c_void) == 0 {
            return Ok(None);
        }
        let metrics = TextMetrics { width: out.width, height: out.height, line_count: out.line_count as usize };
        Ok(Some(match kind {
            METRICS => HostTextMeasurement::Metrics(metrics),
            WRAPPED_RAW => HostTextMeasurement::WrappedWithRawWidth { metrics, raw_width: Some(out.raw_width) },
            EXTENTS => HostTextMeasurement::HorizontalExtents { left: out.left, right: out.right },
            _ => HostTextMeasurement::Length(out.length),
        }))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scene {
    Flowchart,
    Sequence,
    State,
    Class,
    GitGraph,
}

impl Scene {
    pub const ALL: [Scene; 5] = [Scene::Flowchart, Scene::Sequence, Scene::State, Scene::Class, Scene::GitGraph];

    pub fn of(diagram_type: &str) -> Option<Scene> {
        match diagram_type {
            "flowchart-v2" | "flowchart" => Some(Scene::Flowchart),
            "sequence" => Some(Scene::Sequence),
            "stateDiagram" | "state" => Some(Scene::State),
            "classDiagram" | "class" => Some(Scene::Class),
            "gitGraph" => Some(Scene::GitGraph),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Scene::Flowchart => "flowchart",
            Scene::Sequence => "sequence",
            Scene::State => "state",
            Scene::Class => "class",
            Scene::GitGraph => "gitGraph",
        }
    }
}

pub fn drawn_types() -> String {
    let names: Vec<&str> = Scene::ALL.iter().map(|s| s.name()).collect();
    match names.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => names.join(""),
    }
}

pub fn layout(source: &str) -> Result<serde_json::Value, String> {
    scene_layout(source, None, std::ptr::null_mut())
}

fn scene_layout(source: &str, measure: Option<MeasureFn>, context: *mut c_void) -> Result<serde_json::Value, String> {
    let mut value = match catch_unwind(AssertUnwindSafe(|| merman_layout(source, measure, context))) {
        Ok(Ok(value)) => value,
        Ok(Err(message)) => match message.split_once("Unsupported diagram type: ") {
            Some((_, rest)) => {
                let diagram_type = rest.split(';').next().unwrap_or(rest).trim();
                serde_json::json!({ "meta": { "diagram_type": diagram_type }, "semantic": {}, "layout": {} })
            }
            None => return Err(message),
        },
        Err(_) => return Err("merman crashed on this diagram".into()),
    };
    let scene = value["meta"]["diagram_type"].as_str().and_then(Scene::of).map(Scene::name);
    value["scene"] = serde_json::json!(scene);
    Ok(value)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    pub line: usize,
    pub diagram_type: Option<String>,
    pub failure: Option<String>,
}

pub fn check(markdown: &str) -> Vec<Checked> {
    use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag};
    let mut out = Vec::new();
    let mut open: Option<(usize, String)> = None;
    for (event, range) in Parser::new_ext(markdown, Options::all()).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) => {
                let mermaid = info.split_whitespace().next().is_some_and(|w| w.eq_ignore_ascii_case("mermaid"));
                open = mermaid.then(|| (range.start, String::new()));
            }
            Event::Text(text) => {
                if let Some((_, source)) = open.as_mut() {
                    source.push_str(&text);
                }
            }
            Event::End(pulldown_cmark::TagEnd::CodeBlock) => {
                if let Some((start, source)) = open.take() {
                    let line = markdown[..start].matches('\n').count() + 1;
                    out.push(check_one(line, &source));
                }
            }
            _ => {}
        }
    }
    out
}

fn check_one(line: usize, source: &str) -> Checked {
    match layout(source) {
        Err(message) => Checked { line, diagram_type: None, failure: Some(format!("diagram error: {message}")) },
        Ok(value) => {
            let diagram_type = value["meta"]["diagram_type"].as_str().unwrap_or("unknown").to_string();
            let failure = value["scene"]
                .is_null()
                .then(|| format!("unsupported diagram type: {diagram_type} (Kanna draws {})", drawn_types()));
            Checked { line, diagram_type: Some(diagram_type), failure }
        }
    }
}

fn merman_layout(source: &str, measure: Option<MeasureFn>, context: *mut c_void) -> Result<serde_json::Value, String> {
    let mut environment = SvgEnvironment::deterministic();
    if let Some(measure) = measure {
        let identity = TextMeasurementProfileIdentity::new(
            MeasurementProfileId::new("kanna.coretext").map_err(|e| format!("{e:?}"))?,
            "1",
        )
        .map_err(|e| format!("{e:?}"))?;
        let host = Arc::new(HostMeasurer { measure, context: context as usize });
        environment = environment.with_text_measurement_policy(TextMeasurementPolicy::host_display(
            identity,
            host,
            TextMeasurementPhase::ALL,
        ));
    }
    let request = SvgRequest { environment, ..Default::default() };
    let output = Renderer::new()
        .render(RenderRequest::layout_json(source, OperationControl::new(), request))
        .map_err(|e| e.to_string())?;
    match output {
        RenderOutput::LayoutJson(Some(layout)) => Ok(layout.layout().clone()),
        _ => Err("No diagram found".into()),
    }
}

#[no_mangle]
pub unsafe extern "C" fn kanna_merman_layout(
    source: *const c_char,
    len: usize,
    measure: Option<MeasureFn>,
    context: *mut c_void,
) -> *mut c_char {
    let bytes = if source.is_null() { &[][..] } else { std::slice::from_raw_parts(source.cast::<u8>(), len) };
    let json = match std::str::from_utf8(bytes) {
        Err(_) => serde_json::json!({ "error": "The diagram is not valid UTF-8" }),
        Ok(text) => scene_layout(text, measure, context).unwrap_or_else(|message| serde_json::json!({ "error": message })),
    };
    CString::new(json.to_string()).map(CString::into_raw).unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn kanna_merman_free(json: *mut c_char) {
    if !json.is_null() {
        drop(CString::from_raw(json));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "# Plan\n\n```mermaid\nflowchart LR\n    A[Start] --> B{Ok?}\n    B -->|yes| C[Done]\n```\n\nText.\n\n```mermaid\nsequenceDiagram\n    participant C as CLI\n    C->>D: open\n    D-->>C: ok\n```\n\n```rust\nfn main() {}\n```\n\n- A list:\n\n  ```mermaid\n  flowchart TD\n      A --> B --> --> C[\n  ```\n\n~~~ Mermaid\nerDiagram\n    CUSTOMER ||--o{ ORDER : places\n~~~\n";

    #[test]
    fn checks_every_mermaid_block_at_its_fence_line() {
        let checked = check(DOC);
        assert_eq!(checked.iter().map(|c| c.line).collect::<Vec<_>>(), [3, 11, 24, 29]);
    }

    #[test]
    fn flowcharts_and_sequence_diagrams_draw() {
        let checked = check(DOC);
        assert_eq!((checked[0].diagram_type.as_deref(), &checked[0].failure), (Some("flowchart-v2"), &None));
        assert_eq!((checked[1].diagram_type.as_deref(), &checked[1].failure), (Some("sequence"), &None));
    }

    #[test]
    fn a_parse_error_fails() {
        let failure = check(DOC)[2].failure.clone().unwrap();
        assert!(failure.starts_with("diagram error: "), "{failure}");
    }

    #[test]
    fn a_type_the_app_does_not_draw_fails() {
        let checked = &check(DOC)[3];
        let diagram_type = checked.diagram_type.clone().unwrap();
        assert_eq!(
            checked.failure.as_deref(),
            Some(format!("unsupported diagram type: {diagram_type} (Kanna draws flowchart, sequence, state, class and gitGraph)").as_str())
        );
    }

    #[test]
    fn every_drawn_type_has_a_scene() {
        for (source, scene) in [
            ("flowchart LR\n A --> B", "flowchart"),
            ("graph TD\n A --> B", "flowchart"),
            ("sequenceDiagram\n A->>B: hi", "sequence"),
            ("stateDiagram-v2\n [*] --> A", "state"),
            ("classDiagram\n class A", "class"),
            ("gitGraph\n commit", "gitGraph"),
        ] {
            assert_eq!(layout(source).unwrap()["scene"], scene, "{source}");
        }
        assert!(layout("pie\n \"a\" : 1").unwrap()["scene"].is_null());
    }

    #[test]
    fn a_doc_without_diagrams_has_nothing_to_check() {
        assert!(check("# Title\n\n```\nflowchart LR\n```\n").is_empty());
    }
}
