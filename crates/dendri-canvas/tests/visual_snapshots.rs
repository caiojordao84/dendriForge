//! Deterministic visual-frame snapshots for the MVP SVG canvas.

use std::path::PathBuf;

use dendri_canvas::SimCanvas;
use dendri_parse::{parse_path, ParsedDocument};
use dendri_sim::SimConfig;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn load_canvas(rel: &str) -> SimCanvas {
    let path = repo().join(rel);
    let doc = parse_path(&path).expect("parse");
    let project = match doc {
        ParsedDocument::Project(p) => p.project,
        ParsedDocument::Program(p) => {
            // Wrap bare ASL as a minimal project for canvas binding defaults.
            dendri_ast::SimulationProject {
                metadata: p.program.metadata.clone(),
                board: dendri_ast::BoardInstance {
                    profile: "arduino-uno-r3".into(),
                    instance_id: "board".into(),
                    layout: Default::default(),
                },
                components: vec![
                    dendri_ast::ComponentInstance {
                        instance_id: "led1".into(),
                        profile: "led".into(),
                        label: None,
                        params: Default::default(),
                        layout: Default::default(),
                    },
                    dendri_ast::ComponentInstance {
                        instance_id: "btn_start".into(),
                        profile: "cmd-pushbutton-no-start".into(),
                        label: None,
                        params: Default::default(),
                        layout: Default::default(),
                    },
                ],
                wiring: vec![
                    dendri_ast::Wire {
                        from: "board.D13".into(),
                        to: "led1.A".into(),
                        signal: Some("digital".into()),
                    },
                    dendri_ast::Wire {
                        from: "board.GND".into(),
                        to: "led1.K".into(),
                        signal: Some("gnd".into()),
                    },
                    dendri_ast::Wire {
                        from: "board.D2".into(),
                        to: "btn_start.NO".into(),
                        signal: Some("digital".into()),
                    },
                    dendri_ast::Wire {
                        from: "board.GND".into(),
                        to: "btn_start.COM".into(),
                        signal: Some("gnd".into()),
                    },
                ],
                sources: vec![],
                asl: p.program,
            }
        }
    };
    SimCanvas::from_project(&project, repo(), &SimConfig::default()).expect("canvas")
}

#[test]
fn c01_button_led_visual_frames() {
    let mut canvas = load_canvas("content/fixtures/canvas/C01-button-led.toml");
    canvas.boot().unwrap();
    // released → LED off
    canvas.release_button();
    canvas.step().unwrap();
    // pressed → LED on
    canvas.press_button();
    canvas.step().unwrap();
    // released → LED off
    canvas.release_button();
    canvas.step().unwrap();

    let got = canvas.frames_snapshot();
    let snap_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots/C01-button-led.visual.txt");
    let expected = std::fs::read_to_string(&snap_path).unwrap_or_default();
    if expected.is_empty() || std::env::var("UPDATE_SNAPSHOTS").is_ok() {
        std::fs::write(&snap_path, &got).unwrap();
    }
    let expected = std::fs::read_to_string(&snap_path).unwrap();
    assert_eq!(got, expected, "visual snapshot mismatch for C01");

    // Rendered SVGs must carry class hooks for the last (released/off) frame.
    let led = canvas.rendered_led();
    assert!(led.contains("led-body"));
    assert!(led.contains("off") || led.contains("--df-state: off") || led.contains("--df-state:off"));
    let btn = canvas.rendered_button();
    assert!(btn.contains("id=\"actuator\""));
    assert!(btn.contains("open"));
    let board = canvas.rendered_board();
    assert!(board.contains("led-pin13"));
}

#[test]
fn press_propagates_to_pin_and_led_render() {
    let mut canvas = load_canvas("content/fixtures/canvas/C01-button-led-logic.toml");
    canvas.boot().unwrap();
    canvas.press_button();
    let frame = canvas.step().unwrap();
    assert!(frame.button_pressed);
    assert!(frame.led_on);
    // Pull-up / active-low: pressed drives the DI pin LOW.
    assert_eq!(canvas.sim.pin(2), false);
    assert_eq!(canvas.sim.pin(13), true);
    let led = canvas.rendered_led();
    assert!(led.contains("on"));
    let btn = canvas.rendered_button();
    assert!(btn.contains("pressed"));
    assert!(btn.contains("closed"));
}
