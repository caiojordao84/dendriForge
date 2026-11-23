//! End-to-end CLI tests for `dendri validate` and `dendri sim`.

use std::process::Command;

#[test]
fn cli_validate_and_sim_runs_two_board_project() {
    let dir = tempfile::tempdir().unwrap();
    let proj_path = dir.path().join("demo.toml");
    let toml = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "CliSimDemo"

[[boards]]
id = "uno"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "cooperative"

[boards.program]
imports = ["std.iec.*", "std.arduino.*"]

[[boards.program.hardware]]
id = "n_u_out"
label = "TX_PIN"
pin = "D8"
kind = "digital_out"

[[boards.program.pous]]
id = "n_u_m"
name = "OB_MAIN"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "call"
id = "n_u_s1"
callee = "DIGITAL_WRITE"
args = [{ value = "TX_PIN" }, { value = "true" }]

[[boards.program.pous.body]]
kind = "wait"
id = "n_u_s2"
duration = "T#20ms"

[[boards]]
id = "uno_ld"
profile = "arduino-uno-r3"
authoring_language = "ladder"
execution = "plc_scan"
scan_period_us = 10000

[boards.program]
imports = ["std.iec.*"]

[[boards.program.hardware]]
id = "n_p_in"
label = "RX_IN"
pin = "D2"
kind = "digital_in"

[[boards.program.hardware]]
id = "n_p_out"
label = "LAMP"
pin = "D13"
kind = "digital_out"

[[boards.program.pous]]
id = "n_p_m"
name = "OB_MAIN"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "network"

[[boards.program.pous.network.rungs]]
id = "n_p_r0"
order = 0

[[boards.program.pous.network.rungs.cells]]
id = "n_p_c0"
row = 0
col = 0
kind = "contact"
contact_kind = "normally_open"
target = "RX_IN"

[[boards.program.pous.network.rungs.cells]]
id = "n_p_c1"
row = 0
col = 1
kind = "coil"
coil_kind = "normal"
target = "LAMP"

[[wires]]
id = "w1"
from = "uno.D8"
to = "uno_ld.D2"
signal = "digital"
"#;
    std::fs::write(&proj_path, toml).unwrap();

    let bin = env!("CARGO_BIN_EXE_dendri");

    let val_out = Command::new(bin)
        .args(["validate", "--report"])
        .arg(&proj_path)
        .output()
        .unwrap();
    assert!(
        val_out.status.success(),
        "validate failed: {}",
        String::from_utf8_lossy(&val_out.stderr)
    );

    let sim_out = Command::new(bin)
        .args(["sim", "--duration-ms", "50", "--seed", "7"])
        .arg(&proj_path)
        .output()
        .unwrap();
    assert!(
        sim_out.status.success(),
        "sim failed: {}",
        String::from_utf8_lossy(&sim_out.stderr)
    );
    let stdout = String::from_utf8_lossy(&sim_out.stdout);
    assert!(stdout.contains("sim OK | mode=digital-wire | project=CliSimDemo | time_us=50000"));
    assert!(stdout.contains("board=uno_ld"));
    assert!(stdout.contains("\"D13\": true"));
}

#[test]
fn cli_spice_tracer_bullet_uno_button_resistor_led_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let proj_path = dir.path().join("tracer_bullet.toml");
    let toml = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "CliSpiceTracerBullet"

[[boards]]
id = "uno"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "cooperative"

[boards.program]
imports = ["std.iec.*", "std.arduino.*"]

[[boards.program.hardware]]
id = "n_btn"
label = "BTN"
pin = "D2"
kind = "digital_in"

[[boards.program.hardware]]
id = "n_led"
label = "LED"
pin = "D13"
kind = "digital_out"

[[boards.program.pous]]
id = "n_main"
name = "OB_MAIN"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "call"
id = "n_wr"
callee = "DIGITAL_WRITE"
args = [{ value = "LED" }, { value = "BTN" }]

[[boards.program.pous.body]]
kind = "wait"
id = "n_w"
duration = "T#5ms"

[[components]]
id = "btn1"
profile = "maker-tactile-4pin"
label = "StartButton"
params = { pressed = "true" }

[[components]]
id = "r_pd"
profile = "resistor"
label = "PullDown10k"
params = { r_ohm = "10000" }

[[components]]
id = "r_led"
profile = "resistor"
label = "Limit220"
params = { r_ohm = "220" }

[[components]]
id = "led1"
profile = "led"
label = "RedLed"
params = { color = "red" }

[[wires]]
id = "w1"
from = "uno.5V"
to = "btn1.1"
signal = "power"

[[wires]]
id = "w2"
from = "btn1.2"
to = "uno.D2"
signal = "digital"

[[wires]]
id = "w3"
from = "uno.D2"
to = "r_pd.1"
signal = "digital"

[[wires]]
id = "w4"
from = "r_pd.2"
to = "uno.GND"
signal = "ground"

[[wires]]
id = "w5"
from = "uno.D13"
to = "r_led.1"
signal = "digital"

[[wires]]
id = "w6"
from = "r_led.2"
to = "led1.A"
signal = "digital"

[[wires]]
id = "w7"
from = "led1.K"
to = "uno.GND"
signal = "ground"
"#;
    std::fs::write(&proj_path, toml).unwrap();

    let bin = env!("CARGO_BIN_EXE_dendri");
    let sim_out = Command::new(bin)
        .args(["sim", "--spice", "--duration-ms", "20"])
        .arg(&proj_path)
        .output()
        .unwrap();
    assert!(
        sim_out.status.success(),
        "SPICE CLI Tracer Bullet failed: {}",
        String::from_utf8_lossy(&sim_out.stderr)
    );
    let stdout = String::from_utf8_lossy(&sim_out.stdout);
    assert!(
        stdout.contains("sim OK | mode=spice-mna | project=CliSpiceTracerBullet | time_us=20000")
    );
    assert!(stdout.contains("\"D13\": true"));
}
