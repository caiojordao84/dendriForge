//! DendriForge iced editor — Caio iced 0.13 prototype (visual SoT) + real sim.
//!
//! UI chrome/views from `docs/canvas-integration/{main,theme}.rs`.
//! Scan/press/reset wired through `dendri-canvas::SimCanvas` on C01-button-led.

mod theme;

use std::path::PathBuf;
use std::time::Duration;

use dendri_canvas::SimCanvas;
use dendri_parse::{parse_path, ParsedDocument};
use dendri_sim::SimConfig;
use iced::{
    alignment, mouse,
    widget::{
        self, button, canvas, checkbox, column, container, row, scrollable, svg, text, Space,
    },
    Color, Element, Font, Length, Point, Rectangle, Renderer, Size, Subscription, Theme,
};
use theme as ui;

const PROJECT: &str = r####"[metadata]
asl_version = "0.0.1"
project_name = "C01-button-led"
editor = "dendriForge"
author = "dendriForge Priority 4"
notes = "Uno + LED module with integrated 220-ohm SMD resistor + normally open pushbutton. LED on while button is pressed."

[board]
profile = "arduino-uno-r3"
instance_id = "board"

[[components]]
instance_id = "led1"
profile = "led"
label = "Status LED"

[components.params]
color = "red"
integrated_resistance = 220

[[components]]
instance_id = "btn_start"
profile = "cmd-pushbutton-no-start"
label = "Start button"

[components.params]

[[wiring]]
from = "board.D13"
to = "led1.A"
signal = "digital"

[[wiring]]
from = "board.GND"
to = "led1.K"
signal = "gnd"

[[wiring]]
from = "board.D2"
to = "btn_start.NO"
signal = "digital"

[[wiring]]
from = "board.GND"
to = "btn_start.COM"
signal = "gnd"

[asl]
functions = []

[asl.metadata]
asl_version = "0.0.1"
project_name = "C01-button-led"
editor = "dendriForge"
author = "dendriForge Priority 4"

[[asl.hardware]]
pin = 2
hw_type = "digital"
io_mode = "DI"
label = "btnPin"
target_role = "digital-in"
pull = "pullup"

[[asl.hardware]]
pin = 13
hw_type = "digital"
io_mode = "DO"
label = "ledPin"
target_role = "digital-out"

[[asl.data]]
name = "pressed"
type = "BOOL"
scope = "VAR"
init = "FALSE"
unit = "-"

[[asl.setup]]
kind = "call"
name = "CFG_DIGITAL_INPUT"
args = [{ kind = "var", name = "btnPin" }]

[[asl.setup]]
kind = "call"
name = "CFG_DIGITAL_OUTPUT"
args = [{ kind = "var", name = "ledPin" }]

[[asl.setup]]
kind = "call"
name = "SET_BOOL"
args = [{ kind = "var", name = "ledPin" }, { kind = "bool", value = false }]

[[asl.loop]]
kind = "assign"
target = "pressed"

[asl.loop.value]
kind = "unary"
op = "not"

[asl.loop.value.expr]
kind = "call"
name = "GET_BOOL"
args = [{ kind = "var", name = "btnPin" }]

[[asl.loop]]
kind = "call"
name = "SET_BOOL"
args = [{ kind = "var", name = "ledPin" }, { kind = "var", name = "pressed" }]
"####;
const SCHEMATIC_OFF: &str = r####"<svg class="w-full max-w-[1100px] max-h-[75vh] drop-shadow-2xl overflow-visible pointer-events-auto" height="620" viewBox="0 0 960 620" width="960" xmlns="http://www.w3.org/2000/svg">
<defs>
<!-- Filter and gradients -->
<filter height="140%" id="shadowGlow" width="140%" x="-20%" y="-20%">
<feDropShadow dx="0" dy="6" flood-color="#000000" flood-opacity="0.6" stdDeviation="6"></feDropShadow>
</filter>
<filter height="200%" id="df-glow" width="200%" x="-50%" y="-50%">
<feGaussianBlur in="SourceGraphic" result="blur" stdDeviation="3.5"></feGaussianBlur>
<feMerge>
<feMergeNode in="blur"></feMergeNode>
<feMergeNode in="SourceGraphic"></feMergeNode>
</feMerge>
</filter>
<!-- Component Styles -->
<style>
      /* Scoped LED styles */
      .led-cls-1 { fill: #2c3e50; }
      .led-cls-2 { fill: #a0aaaf; }
      .led-cls-3 { fill: #c0392b; }
      .led-cls-4 { fill: #e3e8eb; fill-opacity: 0.2; mix-blend-mode: screen; }
      .led-cls-5 { fill: #fff; opacity: 0.8; }
      .led-cls-6 { fill: #7f8c8d; }
      .led-cls-7 { fill: #34495e; }
      .led-cls-8 { font-family: 'Source Sans 3', sans-serif; font-size: 12px; fill: #ebebeb; }
      #led-body { transition: fill 0.2s ease, opacity 0.2s ease, filter 0.2s ease; }
      #led-body.off { opacity: 0.35 !important; fill: #c0392b !important; filter: none !important; }
      #led-body.on { opacity: 1.0 !important; fill: #ef4444 !important; filter: url(#df-glow) !important; }

      /* Scoped Button styles */
      .btn-cls-1 { fill: #2c3e50; }
      .btn-cls-2 { fill: #7f8c8d; stroke: #566573; stroke-width: 1.134; }
      .btn-cls-3 { fill: #27ae60; transition: transform 0.15s ease, fill 0.15s ease; }
      .btn-cls-4 { fill: none; stroke: #ecf0f1; stroke-width: 1.512; }
      .btn-cls-5 { font-family: Roboto, sans-serif; font-size: 6.047px; font-weight: 900; fill: #ecf0f1; text-anchor: middle; }
      .btn-cls-6 { fill: #1a252f; }
      #cmd-pushbutton-no-start { cursor: pointer; }
      #cmd-pushbutton-no-start:hover .btn-cls-3 { fill: #2ecc71; }
      #cmd-pushbutton-no-start.pressed #actuator circle,
      #cmd-pushbutton-no-start:active .btn-cls-3 { fill: #10b981; filter: drop-shadow(0 0 6px #10b981); }
    
.digital{fill:#00e0ff}.analog{fill:#abe338}.power{fill:#fc8181}.ground{fill:#a0aec0}.icsp{fill:#94d2bd}
</style>
</defs>
<!-- ============================== -->
<!-- ARDUINO UNO R3 BOARD GRAPHIC   -->
<!-- ============================== -->
<g id="arduino-uno" transform="translate(60, 140) scale(1.7)">
<g id="board" transform="matrix(1.333334,0,0,1.333334,15.459601,0)">
<path d="M 8.437,0 C 7.029,0.178 6.337,1.073 5.984,2.308 l 0.03,146.991 c 0,1.171 0.984,2.071 2.027,2.071 h 183.04 c 1.247,-0.207 1.777,-0.998 1.964,-2.071 l 0.418,-5.02 5.834,-5.845 1.145,-4.081 V 43.337 c -2.397,-2.553 -6.843,-6.997 -6.843,-6.997 L 193.447,3.342 190.108,0.029 8.438,0.001 h 10e-4 z m 40.362,2.413 c 2.477,0 4.484,1.955 4.484,4.368 0,2.412 -2.007,4.368 -4.484,4.368 -2.477,0 -4.484,-1.956 -4.484,-4.368 0,-2.412 2.007,-4.367 4.484,-4.368 z m 144.853,43.432 c 2.47,0 4.471,2.005 4.471,4.476 0,2.472 -2.003,4.476 -4.471,4.476 -2.47,0 -4.471,-2.005 -4.471,-4.476 0,-2.472 2.003,-4.476 4.471,-4.476 z m -0.084,80.025 c 2.308,0 4.18,1.873 4.18,4.184 0,2.311 -1.872,4.183 -4.18,4.183 -2.308,0 -4.179,-1.872 -4.179,-4.183 0,-2.311 1.87,-4.183 4.179,-4.184 z M 45.344,140.195 c 2.308,0 4.179,1.872 4.179,4.183 0,2.311 -1.87,4.183 -4.179,4.184 -2.308,0 -4.18,-1.873 -4.18,-4.184 0,-2.311 1.872,-4.183 4.18,-4.183 z" data-h="53.4" data-w="68.6" id="board-body" style="fill:#017e7b;fill-rule:nonzero"></path>
<g id="g5">
<clipPath id="_clip1">
<path d="M 9.229,0 C 7.829,0.177 7.14,1.065 6.789,2.293 l 0.029,146.016 c 0,1.163 0.979,2.057 2.016,2.057 h 182.013 c 1.24,-0.205 1.767,-0.992 1.953,-2.057 l 0.415,-4.987 5.801,-5.806 1.138,-4.054 V 43.049 c -2.383,-2.536 -6.805,-6.95 -6.805,-6.95 L 193.198,3.32 189.877,0.029 9.227,0.001 h 10e-4 z m 40.135,2.397 c 2.463,0 4.459,1.942 4.459,4.339 0,2.396 -1.996,4.339 -4.459,4.339 -2.463,0 -4.459,-1.943 -4.459,-4.339 0,-2.396 1.996,-4.338 4.459,-4.339 z m 144.04,43.144 c 2.456,0 4.446,1.991 4.446,4.446 0,2.456 -1.991,4.446 -4.446,4.446 -2.456,0 -4.446,-1.991 -4.446,-4.446 0,-2.456 1.991,-4.446 4.446,-4.446 z m -0.083,79.494 c 2.295,0 4.156,1.861 4.156,4.156 0,2.295 -1.861,4.155 -4.156,4.155 -2.295,0 -4.155,-1.86 -4.155,-4.155 0,-2.295 1.86,-4.155 4.155,-4.156 z m -147.392,14.23 c 2.295,0 4.155,1.86 4.155,4.155 0,2.295 -1.86,4.155 -4.155,4.156 -2.295,0 -4.156,-1.861 -4.156,-4.156 0,-2.295 1.861,-4.155 4.156,-4.155 z" id="path1"></path>
</clipPath>
<g clip-path="url(#_clip1)" id="g4">
<path d="m 18.01,79.33 -0.032,0.032 c 0,-42.553 58.722,-60.566 87.339,-15.104 30.207,-45.987 86.076,-26.153 86.076,15.024 0,41.177 -56.114,59.993 -86.044,15.104 C 76.469,137.678 18.01,121.843 18.01,79.33 Z m 15.351,0 c 0,30.676 42.959,38.865 63.531,0 -20.167,-40.367 -63.531,-30.685 -63.531,0 z m 42.028,-4.309 v 8.481 H 49.657 v -8.481 z m 38.412,4.309 c 17.64,39.269 62.442,29.945 62.577,0.063 0.135,-29.882 -44.405,-41.225 -62.577,-0.063 z m 29.23,4.046 h -8.49 v -9.014 h 8.49 v -8.49 h 9.022 v 8.49 h 8.49 v 9.014 h -8.49 v 8.49 h -9.022 z" id="path2" style="fill:#174d4f"></path>
<path d="m 15.37,76.689 c 0,42.513 58.459,58.348 87.339,15.056 29.929,44.89 86.044,26.073 86.044,-15.104 0,-41.177 -55.867,-61.012 -86.076,-15.024 C 74.06,16.154 15.338,34.169 15.338,76.721 Z m 15.351,0 c 0,-30.685 43.363,-40.367 63.531,0 -20.573,38.865 -63.531,30.676 -63.531,0 z m 80.44,0 c 18.173,-41.162 62.712,-29.818 62.577,0.063 -0.135,29.882 -44.937,39.206 -62.577,-0.063 z" id="path3" style="fill:#ffffff;fill-rule:nonzero;stroke:#000000;stroke-width:1.92px"></path>
<path d="m 140.389,80.735 h -8.49 v -9.014 h 8.49 v -8.49 h 9.022 v 8.49 h 8.49 v 9.014 h -8.49 v 8.49 h -9.022 z" id="path4" style="fill:#ffffff;fill-rule:nonzero"></path>
</g>
</g>
</g><g id="header-digital" transform="matrix(1.333334,0,0,1.333334,15.459601,0)">
<text id="text5" style="font-style:italic;font-weight:700;font-size:5.867px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="106.555" y="22.802">DIGIT<tspan id="tspan5" x="120.758 124.491" y="22.802 22.802">AL</tspan> (PWM = ~)</text>
<g id="header-i2c">
<rect height="16.790001" id="header-i2c-decoration" style="fill:#ffffff" width="15.101" x="61.868999" y="0"></rect>
<circle class="digital" cx="66.203003" cy="4.6149998" data-direction="bidirectional" data-i2c="SCL" data-pin="19" data-type="digital" data-voltage="5V" id="d19-scl" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g6" transform="rotate(-90,67.453856,14.638998)">
<text id="text6" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#017e7b" x="67.454002" y="14.639">SCL</text>
</g>
<circle class="digital" cx="73.005997" cy="4.6149998" data-direction="bidirectional" data-i2c="SDA" data-pin="18" data-type="digital" data-voltage="5V" id="d18-sda" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g7" transform="rotate(-90,74.402422,14.975962)">
<text id="text7" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#017e7b" x="74.402" y="14.976">SDA</text>
</g>
</g>
<circle class="digital" cx="93.415001" cy="4.6149998" data-direction="bidirectional" data-pin="13" data-type="digital" data-voltage="5V" id="D13 / LED / SCK" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g8" transform="rotate(-90,94.666454,12.59599)">
<text id="text8" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="94.666" y="12.596">13</text>
</g>
<circle class="digital" cx="100.218" cy="4.6149998" data-direction="bidirectional" data-pin="12" data-type="digital" data-voltage="5V" id="D12 / MISO" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g9" transform="rotate(-90,102.15535,12.532894)">
<text id="text9" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="102.155" y="12.533">12</text>
</g>
<circle class="digital" cx="107.486" cy="4.6149998" data-direction="bidirectional" data-pin="11" data-pwm="true" data-type="digital" data-voltage="5V" id="D11 PWM / MOSI" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g10" transform="rotate(-90,108.75584,14.494293)">
<text id="text10" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="108.756" y="14.494">~11</text>
</g>
<circle class="digital" cx="114.29" cy="4.6149998" data-direction="bidirectional" data-pin="10" data-pwm="true" data-type="digital" data-voltage="5V" id="D10 PWM / SS" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g11" transform="rotate(-90,115.54094,14.855797)">
<text id="text11" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="115.541" y="14.856">~10</text>
</g>
<circle class="digital" cx="121.71" cy="4.6149998" data-direction="bidirectional" data-pin="9" data-pwm="true" data-type="digital" data-voltage="5V" id="D9 PWM" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g12" transform="rotate(-90,122.96093,12.815884)">
<text id="text12" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="122.961" y="12.816">~9</text>
</g>
<circle class="digital" cx="128.513" cy="4.6149998" data-direction="bidirectional" data-pin="8" data-type="digital" data-voltage="5V" id="D8" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g13" transform="rotate(-90,129.76408,10.54699)">
<text id="text13" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="129.76401" y="10.547">8</text>
</g>
<circle class="digital" cx="140.021" cy="4.6149998" data-direction="bidirectional" data-pin="7" data-type="digital" data-voltage="5V" id="D7" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g14" transform="rotate(-90,141.27274,10.700759)">
<text id="text14" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="141.27299" y="10.701">7</text>
</g>
<circle class="digital" cx="146.825" cy="4.6149998" data-direction="bidirectional" data-pin="6" data-pwm="true" data-type="digital" data-voltage="5V" id="D6 PWM" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g15" transform="rotate(-90,148.07589,12.756521)">
<text id="text15" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="148.076" y="12.757">~6</text>
</g>
<circle class="digital" cx="153.62801" cy="4.6149998" data-direction="bidirectional" data-pin="5" data-pwm="true" data-type="digital" data-voltage="5V" id="D5 PWM" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g16" transform="rotate(-90,154.86099,12.81754)">
<text id="text16" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="154.86099" y="12.818">~5</text>
</g>
<circle class="digital" cx="160.431" cy="4.6149998" data-direction="bidirectional" data-pin="4" data-type="digital" data-voltage="5V" id="D4" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g17" transform="rotate(-90,161.68219,10.506527)">
<text id="text17" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="161.68201" y="10.507">4</text>
</g>
<circle class="digital" cx="167.23399" cy="4.6149998" data-direction="bidirectional" data-interrupt="true" data-pin="3" data-pwm="true" data-type="digital" data-voltage="5V" id="D3" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g18" transform="rotate(-90,168.48534,12.806889)">
<text id="text18" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="168.485" y="12.807">~3</text>
</g>
<circle class="digital" cx="174.037" cy="4.6149998" data-direction="bidirectional" data-interrupt="true" data-pin="2" data-type="digital" data-voltage="5V" id="D2" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g19" transform="rotate(-90,175.30654,10.515122)">
<text id="text19" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="175.30701" y="10.515">2</text>
</g>
<circle class="digital" cx="180.84" cy="4.6149998" data-direction="output" data-pin="1" data-type="digital" data-uart="TX" data-voltage="5V" id="D1 / TX" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g20" transform="rotate(-90,182.09164,18.240896)">
<text id="text20" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="182.092" y="18.240999">TX0&gt;1</text>
</g>
<circle class="digital" cx="187.64301" cy="4.6149998" data-direction="input" data-pin="0" data-type="digital" data-uart="RX" data-voltage="5V" id="D0 / RX" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g21" transform="rotate(-90,188.89479,18.800071)">
<text id="text21" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="188.895" y="18.799999">RX0&lt;0</text>
</g>
</g><g id="header-analog" transform="matrix(1.333334,0,0,1.333334,15.459601,0)">
<text id="text22" style="font-style:italic;font-weight:700;font-size:5.867px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="152.838" y="128.144">ANALOG</text>
<circle class="analog" cx="149.65401" cy="144.567" data-adc="true" data-direction="bidirectional" data-pin="14" data-type="analog" data-voltage="5V" id="A0" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g23" transform="rotate(-90,150.90443,141.24673)">
<text id="text23" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="150.90401" y="141.24699">A0</text>
</g>
<circle class="analog" cx="156.457" cy="144.567" data-adc="true" data-direction="bidirectional" data-pin="15" data-type="analog" data-voltage="5V" id="A1" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g24" transform="rotate(-90,157.72635,141.24573)">
<text id="text24" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="157.726" y="141.246">A1</text>
</g>
<circle class="analog" cx="163.25999" cy="144.567" data-adc="true" data-direction="bidirectional" data-pin="16" data-type="analog" data-voltage="5V" id="A2" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g25" transform="rotate(-90,164.52923,141.24664)">
<text id="text25" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="164.52901" y="141.24699">A2</text>
</g>
<circle class="analog" cx="170.063" cy="144.567" data-adc="true" data-direction="bidirectional" data-pin="17" data-type="analog" data-voltage="5V" id="A3" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g26" transform="rotate(-90,171.31443,141.24592)">
<text id="text26" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="171.314" y="141.246">A3</text>
</g>
<circle class="analog" cx="176.867" cy="144.567" data-adc="true" data-direction="bidirectional" data-i2c="SDA" data-pin="18" data-type="analog" data-voltage="5V" id="A4 / SDA" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g27" transform="rotate(-90,177.56108,141.24625)">
<text id="text27" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="177.561" y="141.246">A4 SDA</text>
</g>
<circle class="analog" cx="183.67" cy="144.567" data-adc="true" data-direction="bidirectional" data-i2c="SCL" data-pin="19" data-type="analog" data-voltage="5V" id="A5 / SCL" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g28" transform="rotate(-90,184.92045,141.24625)">
<text id="text28" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="184.92" y="141.246">A5 SCL</text>
</g>
</g><g id="header-power" transform="matrix(1.333334,0,0,1.333334,15.459601,0)">
<text id="text29" style="font-style:italic;font-weight:700;font-size:5.867px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="106.555" y="127.99">POWER</text>
<circle class="pin power" cx="86.612" cy="144.567" data-direction="input" data-pin="NC" data-type="power" data-voltage="0V" id="NC" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g30" transform="rotate(-90,87.862673,141.24683)">
<text id="text30" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ff0021" x="87.862999" y="141.24699">X</text>
</g>
<circle class="pin power" cx="93.416" cy="144.567" data-direction="output" data-pin="IOREF" data-type="power" data-voltage="5V" id="IOREF" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g31" transform="rotate(-90,94.666455,141.24655)">
<text id="text31" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="94.666" y="141.24699">IOREF</text>
</g>
<circle class="pin power" cx="101.237" cy="144.567" data-direction="input" data-pin="RESET" data-type="power" data-voltage="5V" id="RESET" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g32" transform="rotate(-90,102.48805,141.24628)">
<text id="text32" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="102.488" y="141.246">RESET</text>
</g>
<circle class="pin power" cx="108.04" cy="144.567" data-direction="output" data-pin="3V3" data-type="power" data-voltage="3.3V" id="3V3" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g33" transform="rotate(-90,109.29093,141.24601)">
<text id="text33" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="109.291" y="141.246">3.3 V</text>
</g>
<circle class="pin power" cx="114.844" cy="144.567" data-direction="output" data-pin="5V" data-type="power" data-voltage="5V" id="5V" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g34" transform="rotate(-90,116.0763,141.24647)">
<text id="text34" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="116.076" y="141.246">5 V</text>
</g>
<circle class="pin ground" cx="121.647" cy="144.567" data-direction="input" data-pin="GND" data-type="ground" data-voltage="0V" id="GND" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g35" transform="rotate(-90,122.89723,141.24664)">
<text id="text35" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="122.897" y="141.24699">GND</text>
</g>
<circle class="pin ground" cx="128.45" cy="144.567" data-direction="input" data-pin="GND" data-type="ground" data-voltage="0V" id="circle35" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g36" transform="rotate(-90,129.70119,141.24637)">
<text id="text36" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="129.701" y="141.246">GND</text>
</g>
<circle class="pin power" cx="135.25301" cy="144.567" data-direction="input" data-pin="VIN" data-type="power" data-voltage="7V-12V" id="VIN" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g37" transform="rotate(-90,136.50425,141.2461)">
<text id="text37" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="136.504" y="141.246">VIN</text>
</g>
<circle class="pin power" cx="79.808998" cy="4.6149998" data-direction="input" data-pin="AREF" data-type="power" data-voltage="5V" id="AREF" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g38" transform="rotate(-90,81.060156,17.127592)">
<text id="text38" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="81.059998" y="17.128">AREF</text>
</g>
<circle class="pin ground" cx="86.612" cy="4.6149998" data-direction="input" data-pin="GND" data-type="ground" data-voltage="0V" id="circle38" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g39" transform="rotate(-90,87.863305,15.661339)">
<text id="text39" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="87.862999" y="15.661">GND</text>
</g>
</g><g id="icsp-header-main" transform="matrix(1.333334,0,0,1.333334,15.459601,0)">
<circle class="pin icsp" cx="186.998" cy="64.440002" data-icsp="MISO" id="icsp-1-miso" r="1.9346447" style="stroke-width:0.750153"></circle>
<circle class="pin icsp" cx="193.44501" cy="64.440002" data-icsp="VCC" id="icsp-1-vcc" r="1.9346447" style="stroke-width:0.750153"></circle>
<circle class="pin icsp" cx="186.998" cy="71.799004" data-icsp="SCK" id="icsp-1-sck" r="1.9346447" style="stroke-width:0.750153"></circle>
<circle class="pin icsp" cx="193.44501" cy="71.799004" data-icsp="MOSI" id="icsp-1-mosi" r="1.9346447" style="stroke-width:0.750153"></circle>
<circle class="pin icsp" cx="186.998" cy="79.157997" data-icsp="RESET" id="icsp-1-reset" r="1.9346447" style="stroke-width:0.750153"></circle>
<circle class="pin icsp" cx="193.44501" cy="79.157997" data-icsp="GND" id="icsp-1-gnd" r="1.9346447" style="stroke-width:0.750153"></circle>
</g><g id="icsp-header-usb" transform="matrix(1.333334,0,0,1.333334,15.459601,0)">
<circle class="pin icsp" cx="65.183998" cy="21.969999" data-icsp="MISO" id="icsp-2-miso" r="1.9346447" style="stroke-width:0.750153"></circle>
<circle class="pin icsp" cx="65.183998" cy="28.417" data-icsp="VCC" id="icsp-2-vcc" r="1.9346447" style="stroke-width:0.750153"></circle>
<circle class="pin icsp" cx="57.824001" cy="21.969999" data-icsp="SCK" id="icsp-2-sck" r="1.9346447" style="stroke-width:0.750153"></circle>
<circle class="pin icsp" cx="57.824001" cy="28.417" data-icsp="MOSI" id="icsp-2-mosi" r="1.9346447" style="stroke-width:0.750153"></circle>
<circle class="pin icsp" cx="50.465" cy="21.969999" data-icsp="RESET" id="icsp-2-reset" r="1.9346447" style="stroke-width:0.750153"></circle>
<circle class="pin icsp" cx="50.465" cy="28.417" data-icsp="GND" id="icsp-2-gnd" r="1.9346447" style="stroke-width:0.750153"></circle>
</g><circle class="led led-orange" cx="131.80901" cy="40.933357" data-linked-pin="13" data-state="off" fill="#5b3516" id="led-pin13" r="2.5795276" style="stroke-width:1.0002"></circle><circle class="led led-yellow" cx="131.80901" cy="61.857365" data-linked-pin="1" data-state="off" id="led-tx" r="2.5795276" style="stroke-width:1.0002"></circle><circle class="led led-yellow" cx="131.80901" cy="70.905373" data-linked-pin="0" data-state="off" id="led-rx" r="2.5795276" style="stroke-width:1.0002"></circle><circle class="led led-green" cx="246.02504" cy="62.138699" data-state="on" id="led-power" r="2.5795276" style="stroke-width:1.0002"></circle><g class="button" id="reset-button" transform="matrix(1.333334,0,0,1.333334,15.459601,0)">
<rect height="13.817" id="reset-button-body" style="fill:#ebebeb" width="13.817" x="17.174999" y="3.415"></rect>
<circle cx="24.084" cy="10.323" id="reset-button-button" r="4.487" style="fill:#931515"></circle>
</g><g class="connector connector-usb" id="usb-connector" transform="matrix(1.333334,0,0,1.333334,4.1068237,0)">
<circle cx="34.865124" cy="24.698" id="path5" r="3.259841" style="clip-rule:evenodd;fill:#d7dadb;fill-opacity:1;fill-rule:evenodd;stroke:none;stroke-width:0.960811;stroke-linecap:square;stroke-linejoin:round;paint-order:markers fill stroke"></circle><circle cx="34.865124" cy="58.71373" id="circle5" r="3.259841" style="clip-rule:evenodd;fill:#d7dadb;fill-opacity:1;fill-rule:evenodd;stroke:none;stroke-width:0.960811;stroke-linecap:square;stroke-linejoin:round;paint-order:markers fill stroke"></circle><rect height="34.015732" id="usb-connector-body" style="clip-rule:evenodd;fill:#585858;fill-rule:evenodd;stroke-width:1.16228" width="45.354309" x="-3.0801163" y="24.698"></rect></g><g class="connector connector-power" id="power-jack" transform="matrix(1.333334,0,0,1.333334,0.00133375,0)">
<circle cx="13.681" cy="126.589" id="power-jack-outer" r="7.566"></circle>
<circle cx="13.681" cy="126.589" id="power-jack-inner" r="4.3790002" style="fill:#404040"></circle>
<path d="m 7.716,137.213 v 1.094 c 0,0.371 -0.271,0.67 -0.608,0.67 H 0.607 c -0.337,0 -0.608,-0.298 -0.608,-0.67 v -23.725 c 0,-0.371 0.271,-0.67 0.608,-0.67 h 6.501 c 0.337,0 0.608,0.298 0.608,0.67 v 1.092 h 29.665 v 21.539 z" id="power-jack-shell" style="fill:#333333"></path>
</g><rect height="11.893339" id="crystal-16mhz" style="fill:#ffffff;stroke-width:1.33333" width="34.818687" x="79.158302" y="97.818718"></rect><g id="chip-atmega328p" transform="matrix(1.333334,0,0,1.333334,15.459601,0)">
<path d="m 87.854,90.003 h 99.455 c 0.399,0 0.719,0.439 0.719,0.983 v 26.476 c 0,0.544 -0.322,0.983 -0.719,0.983 H 87.854 c -0.399,0 -0.719,-0.439 -0.719,-0.983 V 90.986 c 0,-0.544 0.322,-0.983 0.719,-0.983 z" id="chip-atmega328p-base" style="fill:#1a1a1a;fill-rule:nonzero"></path>
<rect height="20.802999" id="chip-atmega328p-body" style="fill:#2c3034" width="100.776" x="87.194" y="93.821999"></rect>
<path d="m 182.811,114.92 h 2.824 v 1.347 h -2.824 z m -7.316,0 h 4.299 v 1.347 h -4.299 z m -7.316,0 h 4.299 v 1.347 h -4.299 z m -7.316,0 h 4.299 v 1.347 h -4.299 z m -7.316,0 h 4.299 v 1.347 h -4.299 z m -7.316,0 h 4.299 v 1.347 h -4.299 z m -7.316,0 h 4.299 v 1.347 h -4.299 z m -7.316,0 h 4.299 v 1.347 h -4.299 z m -7.316,0 h 4.299 v 1.347 h -4.299 z m -7.316,0 h 4.299 v 1.347 h -4.299 z m -7.316,0 h 4.299 v 1.347 h -4.299 z m -7.316,0 h 4.299 v 1.347 h -4.299 z m -7.316,0 h 4.299 v 1.347 h -4.299 z m -5.734,0 h 3.137 v 1.347 h -3.137 z m 93.287,-22.15 h 3.137 v 1.347 h -3.137 z m -6.94,0 h 4.299 v 1.347 h -4.299 z m -7.459,0 h 4.299 v 1.347 h -4.299 z m -7.209,0 h 4.299 v 1.347 h -4.299 z m -7.417,0 h 4.299 v 1.347 h -4.299 z m -7.252,0 h 4.299 v 1.347 h -4.299 z m -7.48,0 h 4.299 v 1.347 h -4.299 z m -7.231,0 h 4.299 v 1.347 h -4.299 z m -7.231,0 h 4.299 v 1.347 h -4.299 z m -7.231,0 h 4.299 v 1.347 h -4.299 z m -7.424,0 h 4.299 v 1.347 h -4.299 z m -7.424,0 h 4.299 v 1.347 h -4.299 z m -7.258,0 h 4.299 v 1.347 h -4.299 z m -5.95,0 h 2.992 v 1.347 h -2.992 z" id="chip-atmega328p-pins" style="fill:#c7c3c2;fill-rule:nonzero"></path>
<text id="text40" style="font-size:5.867px;font-family:RobotoMono-Regular, 'Roboto Mono', monospace;fill:#ffffff" x="89.608002" y="111.832">ATmega328P</text>
</g><g id="chip-atmega16u2" transform="matrix(1.333334,0,0,1.333334,15.459601,0)">
<path d="m 54.875,47.473 v -0.488 h 0.914 v 0.488 z m 0,1.351 v -0.488 h 0.914 v 0.488 z m 0,1.351 v -0.488 h 0.914 v 0.488 z m 0,1.351 v -0.488 h 0.914 v 0.488 z m 0,1.517 v -0.488 h 0.914 v 0.488 z m 0,1.517 v -0.488 h 0.914 v 0.488 z m 0,1.351 v -0.488 h 0.914 v 0.488 z m 0,1.579 v -0.488 h 0.914 v 0.488 z m 3.101,2.6 h -0.488 v -0.914 h 0.488 z m 1.417,0 h -0.488 v -0.914 h 0.488 z m 1.417,0 h -0.488 v -0.914 h 0.488 z m 1.417,0 h -0.488 v -0.914 h 0.488 z m 1.417,0 h -0.488 v -0.914 h 0.488 z m 1.417,0 h -0.488 v -0.914 h 0.488 z m 1.417,0 H 65.99 v -0.914 h 0.488 z m 1.469,0 h -0.488 v -0.914 h 0.488 z m 2.74,-3.2 v 0.488 H 69.773 V 56.89 Z m 0,-1.417 v 0.488 h -0.914 v -0.488 z m 0,-1.417 v 0.488 h -0.914 v -0.488 z m 0,-1.417 v 0.488 h -0.914 v -0.488 z m 0,-1.417 v 0.488 h -0.914 v -0.488 z m 0,-1.359 v 0.488 h -0.914 v -0.488 z m 0,-1.484 v 0.488 h -0.914 v -0.488 z m 0,-1.366 v 0.488 h -0.914 v -0.488 z m -3.222,-2.828 h 0.488 v 0.914 h -0.488 z m -1.473,0 h 0.488 v 0.914 h -0.488 z m -1.473,0 h 0.488 v 0.914 h -0.488 z m -1.341,0 h 0.488 v 0.914 h -0.488 z m -1.341,0 h 0.488 v 0.914 h -0.488 z m -1.444,0 h 0.488 v 0.914 h -0.488 z m -1.444,0 h 0.488 v 0.914 h -0.488 z m -1.444,0 h 0.488 v 0.914 h -0.488 z" id="chip-atmega16u2-pins" style="fill:#d7dadb"></path>
<rect height="13.991" id="chip-atmega16u2-body" style="fill:#2c3034" width="13.991" x="55.787998" y="45.139"></rect>
<text id="text41" style="font-size:3.52px;font-family:RobotoMono-Regular, 'Roboto Mono', monospace;fill:#ffffff" x="52.290001" y="64.348">ATmega16U2</text>
</g><g id="voltage-regulator" transform="matrix(1.333334,0,0,1.333334,15.459601,0)">
<path d="m 23.464,92.423 h 9.432 v 17.07 h -9.432 z m 3.78,-21.051 h 3.783 v 7.982 h -3.783 z m 38.835,31.906 h 3.783 v 7.982 H 66.079 Z M 56.025,89.093 h 7.98 v 8.024 h -7.98 z" id="voltage-regulator-chips" style="fill:#53555f;fill-rule:nonzero"></path>
<path d="m 32.896,108.401 v -1.755 h 3.828 v 1.755 z m 0,-6.565 v -1.755 h 3.828 v 1.755 z m 0,-6.312 v -1.755 h 3.828 v 1.755 z m -15.068,9.755 v -8.642 h 5.636 v 8.642 z M 31.027,73.198 v -0.987 h 2.021 v 0.987 z m 0,5.377 v -0.987 h 2.021 v 0.987 z m -5.804,-2.821 v -0.987 h 2.021 v 0.987 z m 44.638,29.348 v -0.987 h 2.021 v 0.987 z m 0,5.377 v -0.987 h 2.021 v 0.987 z m -5.804,0 v -0.987 h 2.021 v 0.987 z m 0,-2.821 v -0.987 h 2.021 v 0.987 z m 0,-2.557 v -0.987 h 2.021 v 0.987 z m -0.053,-9.012 v -0.987 h 1.786 v 0.987 z m 0,-1.818 v -0.987 h 1.786 v 0.987 z m 0,-1.818 v -0.987 h 1.786 v 0.987 z m 0,-1.818 v -0.987 h 1.786 v 0.987 z M 54.238,96.1 v -0.987 h 1.786 V 96.1 Z m 0,-1.839 v -0.987 h 1.786 v 0.987 z m 0,-1.839 v -0.987 h 1.786 v 0.987 z m 0,-1.839 v -0.987 h 1.786 v 0.987 z" id="voltage-regulator-pins" style="fill:#d7dadb;fill-rule:nonzero"></path>
</g>
</g>
<!-- ============================================== -->
<!-- EXTERNAL BREADBOARD / OFF-BOARD COMPONENTS     -->
<!-- ============================================== -->
<!-- 1. MODULAR 5mm LED WITH INTEGRATED RESISTOR (LED_ongrid.svg) -->
<g id="led-5mm" transform="translate(680, 100) scale(0.95)">
<g id="leadframe">
<path class="led-cls-1" d="M144 36 L144 108 C144 127.869 127.869 144 108 144 L36 144 C16.131 144 0 127.869 0 108 L0 36 C0 16.131 16.131 0 36 0 L108 0 C127.869 0 144 16.131 144 36 Z" id="path1"></path>
<path class="led-cls-2" d="M141.841 37.079 L141.841 106.918 C141.841 126.191 126.194 141.838 106.921 141.838 L37.082 141.838 C17.809 141.838 2.162 126.191 2.162 106.918 L2.162 37.079 C2.162 17.806 17.809 2.159 37.082 2.159 L106.921 2.159 C126.194 2.159 141.841 17.806 141.841 37.079 Z" id="path2"></path>
</g>
<g id="case">
<circle class="led-cls-1" cx="72.001" cy="71.999" id="frame" r="48.107"></circle>
<circle class="led-cls-3 off" cx="72.001" cy="71.999" data-component-id="led" data-led-type="standard-thru-hole" data-param-color="var(--df-color)" data-param-if-max="var(--df-if-max)" data-param-if-rated="var(--df-if-rated)" data-param-luminous-intensity="var(--df-luminous-intensity)" data-param-power-rated="var(--df-power-rated)" data-param-vf="var(--df-vf)" data-param-vr-max="var(--df-vr-max)" data-param-wavelength-peak="var(--df-wavelength-peak)" id="led-body" r="45.22"></circle>
<ellipse class="led-cls-4" cx="73.504" cy="69.995" id="lens" rx="42.333" ry="41.372"></ellipse>
<path class="led-cls-5" d="M92.04 38.236 C96.884 41.367 97.74 42.698 99.098 43.798 C99.508 44.13 100.955 45.896 101.875 46.828 C102.231 47.188 103.384 48.889 103.51 49.073 C104.283 50.214 105.336 51.406 106.831 54.687 C107.511 56.175 107.967 57.092 108.07 57.299 C108.14 57.439 108.068 57.465 108.773 59.127 C109.078 59.844 108.457 59.529 106.888 60.168 C104.888 60.981 104.839 60.797 102.83 61.597 C101.442 62.149 101.214 61.76 101.146 61.644 C100.805 61.066 99.591 56.923 96.211 52.644 C92.591 48.061 91.113 47.519 89.661 46.26 C87.885 44.718 82.983 42.49 82.375 42.214 C81.738 41.924 81.331 41.875 81.825 40.966 C81.962 40.714 82.199 39.761 82.367 39.379 C82.558 38.946 82.623 38.993 82.812 38.555 C82.851 38.465 82.853 38.135 82.957 37.9 C84.233 35.01 83.992 34.383 84.757 34.7 C85.495 35.007 85.525 34.902 86.251 35.254 C88.229 36.209 88.312 36.026 92.038 38.239 Z" id="reflex"></path>
</g>
<g id="wire-bond">
<g id="anodes">
<circle class="df-pin power led-cls-3" cx="116" cy="124.107" data-pin="A" data-port-direction="input" data-port-type="power" id="A2" r="4"></circle>
<circle class="df-pin power led-cls-3" cx="20" cy="20.107" data-pin="A" data-port-direction="input" data-port-type="power" id="A1" r="4"></circle>
</g>
<g id="cathodes">
<circle class="df-pin ground led-cls-3" cx="124" cy="20.107" data-pin="K" data-port-direction="input" data-port-type="ground" id="K2" r="4"></circle>
<circle class="df-pin ground led-cls-3" cx="20" cy="124.107" data-pin="K" data-port-direction="input" data-port-type="ground" id="K1" r="4"></circle>
</g>
<g>
<rect class="led-cls-6" height="8" width="21.668" x="61.166" y="8.107"></rect>
<rect class="led-cls-7" height="8" width="16" x="64" y="8.107"></rect>
<text class="led-cls-8" transform="matrix(0.429 0 0 0.497 64 14.073)" x="0" xml:space="preserve" y="0"><tspan x="0" y="0">RSMDΩ</tspan></text>
</g>
</g>
</g>
<!-- 2. TACTILE PUSHBUTTON NO (Button_NO.svg) -->
<g aria-label="Pushbutton Switch" class="cursor-pointer" id="tactile-switch" role="button" tabindex="0" transform="translate(680, 290) scale(0.95)">
<g id="cmd-pushbutton-no-start">
<g>
<path class="housing btn-cls-1" d="M11.676 0 L132.324 0 C138.773 0 144 5.227 144 11.676 L144 132.324 C144 138.773 138.773 144 132.324 144 L11.676 144 C5.227 144 0 138.773 0 132.324 L0 11.676 C0 5.227 5.227 0 11.676 0 Z"></path>
</g>
<circle class="bezel btn-cls-2" cx="72" cy="72" r="48"></circle>
<g id="actuator">
<g>
<circle class="btn-cls-3" cx="72" cy="72" r="40"></circle>
</g>
</g>
<g class="open" id="contact">
<polyline class="btn-cls-4" id="contact-arm" points="48,136 64.696,136 75.13,127.652"></polyline>
<line class="btn-cls-4" x1="79.304" x2="96" y1="136" y2="136"></line>
</g>
<text class="btn-cls-5" id="port-NO1" transform="matrix(1.8 0 0 1.8 72 75.72)" x="0" xml:space="preserve" y="0"><tspan x="0" y="0">NO</tspan></text>
<circle class="btn-cls-6" cx="132" cy="132" data-name="Connector 2b" id="Connector_2b" r="4"></circle>
<circle class="btn-cls-6" cx="12" cy="132" data-name="Connector 2a" id="Connector_2a" r="4"></circle>
<circle class="btn-cls-6" cx="132" cy="12" data-name="Connector 1b" id="Connector_1b" r="4"></circle>
<circle class="btn-cls-6" cx="12" cy="12" data-name="Connector 1a" id="Connector_1a" r="4"></circle>
</g>
</g>
<!-- MCU pull-up callout (no discrete pull resistor on canvas) -->
<g id="mcu-pullup-note" transform="translate(360, 168)">
<rect fill="#0f172a" fill-opacity="0.85" height="28" rx="4" stroke="#34d399" stroke-width="1" width="168" x="0" y="0"></rect>
<text fill="#6ee7b7" font-family="'JetBrains Mono', monospace" font-size="9" x="8" y="12">D2 · MCU pull-up</text>
<text fill="#94a3b8" font-family="'JetBrains Mono', monospace" font-size="8" x="8" y="22">pressed → active-LOW</text>
</g>
<!-- ============================================== -->
<!-- REALISTIC BENT JUMPER WIRES (SVG BEZIER PATHS) -->
<!-- Button: D2 ↔ GND with MCU INPUT_PULLUP         -->
<!-- LED: D13 ↔ GND, integrated SMD resistance      -->
<!-- ============================================== -->
<g filter="url(#shadowGlow)" id="circuit-wiring" stroke-linecap="round" stroke-linejoin="round">
<!-- 1. WIRE GREEN: Arduino Digital Pin 2 → Button NO (Connector_2a) -->
<path d="M 355.9 147.8 C 355.9 80, 620 70, 650 240 C 665 320, 660 415.4, 691.4 415.4" fill="none" stroke="#22c55e" stroke-width="4.5"></path>
<circle cx="355.9" cy="147.8" fill="#14532d" r="4" stroke="#86efac" stroke-width="1.5"></circle>
<circle cx="691.4" cy="415.4" fill="#14532d" r="3.5" stroke="#86efac" stroke-width="1.5"></circle>
<!-- 2. WIRE BLACK: Button COM (Connector_1a) → Arduino Power GND -->
<path d="M 691.4 301.4 C 640 360, 420 430, 266.8 385.8" fill="none" stroke="#475569" stroke-width="4.5"></path>
<circle cx="691.4" cy="301.4" fill="#1e293b" r="3.5" stroke="#94a3b8" stroke-width="1.5"></circle>
<circle cx="266.8" cy="385.8" fill="#1e293b" r="4" stroke="#94a3b8" stroke-width="1.5"></circle>
<!-- 3. WIRE SKY-BLUE: Pin 13 to External LED Anode A1 -->
<path d="M 218.8 147.8 C 218.8 45, 610 35, 699.0 119.1" fill="none" stroke="#38bdf8" stroke-width="4"></path>
<circle cx="218.8" cy="147.8" fill="#0369a1" r="3.5" stroke="#bae6fd" stroke-width="1.5"></circle>
<circle cx="699.0" cy="119.1" fill="#0369a1" r="3.5" stroke="#bae6fd" stroke-width="1.5"></circle>
<!-- 4. WIRE DASHED SLATE: LED Cathode K1 to Digital Header GND -->
<path d="M 699.0 217.9 C 630 220, 480 210, 360 185 C 270 165, 207.2 165, 207.2 147.8" fill="none" stroke="#64748b" stroke-dasharray="6,4" stroke-width="3.5"></path>
<circle cx="699.0" cy="217.9" fill="#1e293b" r="3.5" stroke="#94a3b8" stroke-width="1.5"></circle>
<circle cx="207.2" cy="147.8" fill="#1e293b" r="3.5" stroke="#94a3b8" stroke-width="1.5"></circle>
</g>
<!-- Current Flow Animated Particle Annotations (WCAG Safe, High Contrast) -->
<g>
<circle cx="520" cy="360" fill="#86efac" r="2.5">
<animate attributename="opacity" dur="1.2s" repeatcount="indefinite" values="0.3;1;0.3"></animate>
</circle>
<circle cx="460" cy="52" fill="#7dd3fc" r="2.5">
<animate attributename="opacity" dur="0.9s" repeatcount="indefinite" values="1;0.2;1"></animate>
</circle>
</g>
</svg>"####;
const SCHEMATIC_ON: &str = r####"<svg class="w-full max-w-[1100px] max-h-[75vh] drop-shadow-2xl overflow-visible pointer-events-auto" height="620" viewBox="0 0 960 620" width="960" xmlns="http://www.w3.org/2000/svg">
<defs>
<!-- Filter and gradients -->
<filter height="140%" id="shadowGlow" width="140%" x="-20%" y="-20%">
<feDropShadow dx="0" dy="6" flood-color="#000000" flood-opacity="0.6" stdDeviation="6"></feDropShadow>
</filter>
<filter height="200%" id="df-glow" width="200%" x="-50%" y="-50%">
<feGaussianBlur in="SourceGraphic" result="blur" stdDeviation="3.5"></feGaussianBlur>
<feMerge>
<feMergeNode in="blur"></feMergeNode>
<feMergeNode in="SourceGraphic"></feMergeNode>
</feMerge>
</filter>
<!-- Component Styles -->
<style>
      /* Scoped LED styles */
      .led-cls-1 { fill: #2c3e50; }
      .led-cls-2 { fill: #a0aaaf; }
      .led-cls-3 { fill: #c0392b; }
      .led-cls-4 { fill: #e3e8eb; fill-opacity: 0.2; mix-blend-mode: screen; }
      .led-cls-5 { fill: #fff; opacity: 0.8; }
      .led-cls-6 { fill: #7f8c8d; }
      .led-cls-7 { fill: #34495e; }
      .led-cls-8 { font-family: 'Source Sans 3', sans-serif; font-size: 12px; fill: #ebebeb; }
      #led-body { transition: fill 0.2s ease, opacity 0.2s ease, filter 0.2s ease; }
      #led-body.off { opacity: 0.35 !important; fill: #c0392b !important; filter: none !important; }
      #led-body.on { opacity: 1.0 !important; fill: #ef4444 !important; filter: url(#df-glow) !important; }

      /* Scoped Button styles */
      .btn-cls-1 { fill: #2c3e50; }
      .btn-cls-2 { fill: #7f8c8d; stroke: #566573; stroke-width: 1.134; }
      .btn-cls-3 { fill: #27ae60; transition: transform 0.15s ease, fill 0.15s ease; }
      .btn-cls-4 { fill: none; stroke: #ecf0f1; stroke-width: 1.512; }
      .btn-cls-5 { font-family: Roboto, sans-serif; font-size: 6.047px; font-weight: 900; fill: #ecf0f1; text-anchor: middle; }
      .btn-cls-6 { fill: #1a252f; }
      #cmd-pushbutton-no-start { cursor: pointer; }
      #cmd-pushbutton-no-start:hover .btn-cls-3 { fill: #2ecc71; }
      #cmd-pushbutton-no-start.pressed #actuator circle,
      #cmd-pushbutton-no-start:active .btn-cls-3 { fill: #10b981; filter: drop-shadow(0 0 6px #10b981); }
    
.digital{fill:#00e0ff}.analog{fill:#abe338}.power{fill:#fc8181}.ground{fill:#a0aec0}.icsp{fill:#94d2bd}
</style>
</defs>
<!-- ============================== -->
<!-- ARDUINO UNO R3 BOARD GRAPHIC   -->
<!-- ============================== -->
<g id="arduino-uno" transform="translate(60, 140) scale(1.7)">
<g id="board" transform="matrix(1.333334,0,0,1.333334,15.459601,0)">
<path d="M 8.437,0 C 7.029,0.178 6.337,1.073 5.984,2.308 l 0.03,146.991 c 0,1.171 0.984,2.071 2.027,2.071 h 183.04 c 1.247,-0.207 1.777,-0.998 1.964,-2.071 l 0.418,-5.02 5.834,-5.845 1.145,-4.081 V 43.337 c -2.397,-2.553 -6.843,-6.997 -6.843,-6.997 L 193.447,3.342 190.108,0.029 8.438,0.001 h 10e-4 z m 40.362,2.413 c 2.477,0 4.484,1.955 4.484,4.368 0,2.412 -2.007,4.368 -4.484,4.368 -2.477,0 -4.484,-1.956 -4.484,-4.368 0,-2.412 2.007,-4.367 4.484,-4.368 z m 144.853,43.432 c 2.47,0 4.471,2.005 4.471,4.476 0,2.472 -2.003,4.476 -4.471,4.476 -2.47,0 -4.471,-2.005 -4.471,-4.476 0,-2.472 2.003,-4.476 4.471,-4.476 z m -0.084,80.025 c 2.308,0 4.18,1.873 4.18,4.184 0,2.311 -1.872,4.183 -4.18,4.183 -2.308,0 -4.179,-1.872 -4.179,-4.183 0,-2.311 1.87,-4.183 4.179,-4.184 z M 45.344,140.195 c 2.308,0 4.179,1.872 4.179,4.183 0,2.311 -1.87,4.183 -4.179,4.184 -2.308,0 -4.18,-1.873 -4.18,-4.184 0,-2.311 1.872,-4.183 4.18,-4.183 z" data-h="53.4" data-w="68.6" id="board-body" style="fill:#017e7b;fill-rule:nonzero"></path>
<g id="g5">
<clipPath id="_clip1">
<path d="M 9.229,0 C 7.829,0.177 7.14,1.065 6.789,2.293 l 0.029,146.016 c 0,1.163 0.979,2.057 2.016,2.057 h 182.013 c 1.24,-0.205 1.767,-0.992 1.953,-2.057 l 0.415,-4.987 5.801,-5.806 1.138,-4.054 V 43.049 c -2.383,-2.536 -6.805,-6.95 -6.805,-6.95 L 193.198,3.32 189.877,0.029 9.227,0.001 h 10e-4 z m 40.135,2.397 c 2.463,0 4.459,1.942 4.459,4.339 0,2.396 -1.996,4.339 -4.459,4.339 -2.463,0 -4.459,-1.943 -4.459,-4.339 0,-2.396 1.996,-4.338 4.459,-4.339 z m 144.04,43.144 c 2.456,0 4.446,1.991 4.446,4.446 0,2.456 -1.991,4.446 -4.446,4.446 -2.456,0 -4.446,-1.991 -4.446,-4.446 0,-2.456 1.991,-4.446 4.446,-4.446 z m -0.083,79.494 c 2.295,0 4.156,1.861 4.156,4.156 0,2.295 -1.861,4.155 -4.156,4.155 -2.295,0 -4.155,-1.86 -4.155,-4.155 0,-2.295 1.86,-4.155 4.155,-4.156 z m -147.392,14.23 c 2.295,0 4.155,1.86 4.155,4.155 0,2.295 -1.86,4.155 -4.155,4.156 -2.295,0 -4.156,-1.861 -4.156,-4.156 0,-2.295 1.861,-4.155 4.156,-4.155 z" id="path1"></path>
</clipPath>
<g clip-path="url(#_clip1)" id="g4">
<path d="m 18.01,79.33 -0.032,0.032 c 0,-42.553 58.722,-60.566 87.339,-15.104 30.207,-45.987 86.076,-26.153 86.076,15.024 0,41.177 -56.114,59.993 -86.044,15.104 C 76.469,137.678 18.01,121.843 18.01,79.33 Z m 15.351,0 c 0,30.676 42.959,38.865 63.531,0 -20.167,-40.367 -63.531,-30.685 -63.531,0 z m 42.028,-4.309 v 8.481 H 49.657 v -8.481 z m 38.412,4.309 c 17.64,39.269 62.442,29.945 62.577,0.063 0.135,-29.882 -44.405,-41.225 -62.577,-0.063 z m 29.23,4.046 h -8.49 v -9.014 h 8.49 v -8.49 h 9.022 v 8.49 h 8.49 v 9.014 h -8.49 v 8.49 h -9.022 z" id="path2" style="fill:#174d4f"></path>
<path d="m 15.37,76.689 c 0,42.513 58.459,58.348 87.339,15.056 29.929,44.89 86.044,26.073 86.044,-15.104 0,-41.177 -55.867,-61.012 -86.076,-15.024 C 74.06,16.154 15.338,34.169 15.338,76.721 Z m 15.351,0 c 0,-30.685 43.363,-40.367 63.531,0 -20.573,38.865 -63.531,30.676 -63.531,0 z m 80.44,0 c 18.173,-41.162 62.712,-29.818 62.577,0.063 -0.135,29.882 -44.937,39.206 -62.577,-0.063 z" id="path3" style="fill:#ffffff;fill-rule:nonzero;stroke:#000000;stroke-width:1.92px"></path>
<path d="m 140.389,80.735 h -8.49 v -9.014 h 8.49 v -8.49 h 9.022 v 8.49 h 8.49 v 9.014 h -8.49 v 8.49 h -9.022 z" id="path4" style="fill:#ffffff;fill-rule:nonzero"></path>
</g>
</g>
</g><g id="header-digital" transform="matrix(1.333334,0,0,1.333334,15.459601,0)">
<text id="text5" style="font-style:italic;font-weight:700;font-size:5.867px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="106.555" y="22.802">DIGIT<tspan id="tspan5" x="120.758 124.491" y="22.802 22.802">AL</tspan> (PWM = ~)</text>
<g id="header-i2c">
<rect height="16.790001" id="header-i2c-decoration" style="fill:#ffffff" width="15.101" x="61.868999" y="0"></rect>
<circle class="digital" cx="66.203003" cy="4.6149998" data-direction="bidirectional" data-i2c="SCL" data-pin="19" data-type="digital" data-voltage="5V" id="d19-scl" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g6" transform="rotate(-90,67.453856,14.638998)">
<text id="text6" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#017e7b" x="67.454002" y="14.639">SCL</text>
</g>
<circle class="digital" cx="73.005997" cy="4.6149998" data-direction="bidirectional" data-i2c="SDA" data-pin="18" data-type="digital" data-voltage="5V" id="d18-sda" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g7" transform="rotate(-90,74.402422,14.975962)">
<text id="text7" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#017e7b" x="74.402" y="14.976">SDA</text>
</g>
</g>
<circle class="digital" cx="93.415001" cy="4.6149998" data-direction="bidirectional" data-pin="13" data-type="digital" data-voltage="5V" id="D13 / LED / SCK" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g8" transform="rotate(-90,94.666454,12.59599)">
<text id="text8" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="94.666" y="12.596">13</text>
</g>
<circle class="digital" cx="100.218" cy="4.6149998" data-direction="bidirectional" data-pin="12" data-type="digital" data-voltage="5V" id="D12 / MISO" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g9" transform="rotate(-90,102.15535,12.532894)">
<text id="text9" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="102.155" y="12.533">12</text>
</g>
<circle class="digital" cx="107.486" cy="4.6149998" data-direction="bidirectional" data-pin="11" data-pwm="true" data-type="digital" data-voltage="5V" id="D11 PWM / MOSI" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g10" transform="rotate(-90,108.75584,14.494293)">
<text id="text10" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="108.756" y="14.494">~11</text>
</g>
<circle class="digital" cx="114.29" cy="4.6149998" data-direction="bidirectional" data-pin="10" data-pwm="true" data-type="digital" data-voltage="5V" id="D10 PWM / SS" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g11" transform="rotate(-90,115.54094,14.855797)">
<text id="text11" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="115.541" y="14.856">~10</text>
</g>
<circle class="digital" cx="121.71" cy="4.6149998" data-direction="bidirectional" data-pin="9" data-pwm="true" data-type="digital" data-voltage="5V" id="D9 PWM" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g12" transform="rotate(-90,122.96093,12.815884)">
<text id="text12" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="122.961" y="12.816">~9</text>
</g>
<circle class="digital" cx="128.513" cy="4.6149998" data-direction="bidirectional" data-pin="8" data-type="digital" data-voltage="5V" id="D8" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g13" transform="rotate(-90,129.76408,10.54699)">
<text id="text13" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="129.76401" y="10.547">8</text>
</g>
<circle class="digital" cx="140.021" cy="4.6149998" data-direction="bidirectional" data-pin="7" data-type="digital" data-voltage="5V" id="D7" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g14" transform="rotate(-90,141.27274,10.700759)">
<text id="text14" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="141.27299" y="10.701">7</text>
</g>
<circle class="digital" cx="146.825" cy="4.6149998" data-direction="bidirectional" data-pin="6" data-pwm="true" data-type="digital" data-voltage="5V" id="D6 PWM" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g15" transform="rotate(-90,148.07589,12.756521)">
<text id="text15" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="148.076" y="12.757">~6</text>
</g>
<circle class="digital" cx="153.62801" cy="4.6149998" data-direction="bidirectional" data-pin="5" data-pwm="true" data-type="digital" data-voltage="5V" id="D5 PWM" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g16" transform="rotate(-90,154.86099,12.81754)">
<text id="text16" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="154.86099" y="12.818">~5</text>
</g>
<circle class="digital" cx="160.431" cy="4.6149998" data-direction="bidirectional" data-pin="4" data-type="digital" data-voltage="5V" id="D4" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g17" transform="rotate(-90,161.68219,10.506527)">
<text id="text17" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="161.68201" y="10.507">4</text>
</g>
<circle class="digital" cx="167.23399" cy="4.6149998" data-direction="bidirectional" data-interrupt="true" data-pin="3" data-pwm="true" data-type="digital" data-voltage="5V" id="D3" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g18" transform="rotate(-90,168.48534,12.806889)">
<text id="text18" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="168.485" y="12.807">~3</text>
</g>
<circle class="digital" cx="174.037" cy="4.6149998" data-direction="bidirectional" data-interrupt="true" data-pin="2" data-type="digital" data-voltage="5V" id="D2" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g19" transform="rotate(-90,175.30654,10.515122)">
<text id="text19" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="175.30701" y="10.515">2</text>
</g>
<circle class="digital" cx="180.84" cy="4.6149998" data-direction="output" data-pin="1" data-type="digital" data-uart="TX" data-voltage="5V" id="D1 / TX" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g20" transform="rotate(-90,182.09164,18.240896)">
<text id="text20" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="182.092" y="18.240999">TX0&gt;1</text>
</g>
<circle class="digital" cx="187.64301" cy="4.6149998" data-direction="input" data-pin="0" data-type="digital" data-uart="RX" data-voltage="5V" id="D0 / RX" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g21" transform="rotate(-90,188.89479,18.800071)">
<text id="text21" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="188.895" y="18.799999">RX0&lt;0</text>
</g>
</g><g id="header-analog" transform="matrix(1.333334,0,0,1.333334,15.459601,0)">
<text id="text22" style="font-style:italic;font-weight:700;font-size:5.867px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="152.838" y="128.144">ANALOG</text>
<circle class="analog" cx="149.65401" cy="144.567" data-adc="true" data-direction="bidirectional" data-pin="14" data-type="analog" data-voltage="5V" id="A0" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g23" transform="rotate(-90,150.90443,141.24673)">
<text id="text23" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="150.90401" y="141.24699">A0</text>
</g>
<circle class="analog" cx="156.457" cy="144.567" data-adc="true" data-direction="bidirectional" data-pin="15" data-type="analog" data-voltage="5V" id="A1" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g24" transform="rotate(-90,157.72635,141.24573)">
<text id="text24" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="157.726" y="141.246">A1</text>
</g>
<circle class="analog" cx="163.25999" cy="144.567" data-adc="true" data-direction="bidirectional" data-pin="16" data-type="analog" data-voltage="5V" id="A2" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g25" transform="rotate(-90,164.52923,141.24664)">
<text id="text25" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="164.52901" y="141.24699">A2</text>
</g>
<circle class="analog" cx="170.063" cy="144.567" data-adc="true" data-direction="bidirectional" data-pin="17" data-type="analog" data-voltage="5V" id="A3" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g26" transform="rotate(-90,171.31443,141.24592)">
<text id="text26" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="171.314" y="141.246">A3</text>
</g>
<circle class="analog" cx="176.867" cy="144.567" data-adc="true" data-direction="bidirectional" data-i2c="SDA" data-pin="18" data-type="analog" data-voltage="5V" id="A4 / SDA" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g27" transform="rotate(-90,177.56108,141.24625)">
<text id="text27" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="177.561" y="141.246">A4 SDA</text>
</g>
<circle class="analog" cx="183.67" cy="144.567" data-adc="true" data-direction="bidirectional" data-i2c="SCL" data-pin="19" data-type="analog" data-voltage="5V" id="A5 / SCL" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g28" transform="rotate(-90,184.92045,141.24625)">
<text id="text28" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="184.92" y="141.246">A5 SCL</text>
</g>
</g><g id="header-power" transform="matrix(1.333334,0,0,1.333334,15.459601,0)">
<text id="text29" style="font-style:italic;font-weight:700;font-size:5.867px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="106.555" y="127.99">POWER</text>
<circle class="pin power" cx="86.612" cy="144.567" data-direction="input" data-pin="NC" data-type="power" data-voltage="0V" id="NC" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g30" transform="rotate(-90,87.862673,141.24683)">
<text id="text30" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ff0021" x="87.862999" y="141.24699">X</text>
</g>
<circle class="pin power" cx="93.416" cy="144.567" data-direction="output" data-pin="IOREF" data-type="power" data-voltage="5V" id="IOREF" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g31" transform="rotate(-90,94.666455,141.24655)">
<text id="text31" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="94.666" y="141.24699">IOREF</text>
</g>
<circle class="pin power" cx="101.237" cy="144.567" data-direction="input" data-pin="RESET" data-type="power" data-voltage="5V" id="RESET" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g32" transform="rotate(-90,102.48805,141.24628)">
<text id="text32" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="102.488" y="141.246">RESET</text>
</g>
<circle class="pin power" cx="108.04" cy="144.567" data-direction="output" data-pin="3V3" data-type="power" data-voltage="3.3V" id="3V3" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g33" transform="rotate(-90,109.29093,141.24601)">
<text id="text33" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="109.291" y="141.246">3.3 V</text>
</g>
<circle class="pin power" cx="114.844" cy="144.567" data-direction="output" data-pin="5V" data-type="power" data-voltage="5V" id="5V" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g34" transform="rotate(-90,116.0763,141.24647)">
<text id="text34" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="116.076" y="141.246">5 V</text>
</g>
<circle class="pin ground" cx="121.647" cy="144.567" data-direction="input" data-pin="GND" data-type="ground" data-voltage="0V" id="GND" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g35" transform="rotate(-90,122.89723,141.24664)">
<text id="text35" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="122.897" y="141.24699">GND</text>
</g>
<circle class="pin ground" cx="128.45" cy="144.567" data-direction="input" data-pin="GND" data-type="ground" data-voltage="0V" id="circle35" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g36" transform="rotate(-90,129.70119,141.24637)">
<text id="text36" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="129.701" y="141.246">GND</text>
</g>
<circle class="pin power" cx="135.25301" cy="144.567" data-direction="input" data-pin="VIN" data-type="power" data-voltage="7V-12V" id="VIN" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g37" transform="rotate(-90,136.50425,141.2461)">
<text id="text37" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="136.504" y="141.246">VIN</text>
</g>
<circle class="pin power" cx="79.808998" cy="4.6149998" data-direction="input" data-pin="AREF" data-type="power" data-voltage="5V" id="AREF" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g38" transform="rotate(-90,81.060156,17.127592)">
<text id="text38" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="81.059998" y="17.128">AREF</text>
</g>
<circle class="pin ground" cx="86.612" cy="4.6149998" data-direction="input" data-pin="GND" data-type="ground" data-voltage="0V" id="circle38" r="1.9346447" style="stroke-width:0.750153"></circle>
<g id="g39" transform="rotate(-90,87.863305,15.661339)">
<text id="text39" style="font-style:italic;font-weight:700;font-size:3.52px;font-family:Roboto-BoldItalic, Roboto;fill:#ffffff" x="87.862999" y="15.661">GND</text>
</g>
</g><g id="icsp-header-main" transform="matrix(1.333334,0,0,1.333334,15.459601,0)">
<circle class="pin icsp" cx="186.998" cy="64.440002" data-icsp="MISO" id="icsp-1-miso" r="1.9346447" style="stroke-width:0.750153"></circle>
<circle class="pin icsp" cx="193.44501" cy="64.440002" data-icsp="VCC" id="icsp-1-vcc" r="1.9346447" style="stroke-width:0.750153"></circle>
<circle class="pin icsp" cx="186.998" cy="71.799004" data-icsp="SCK" id="icsp-1-sck" r="1.9346447" style="stroke-width:0.750153"></circle>
<circle class="pin icsp" cx="193.44501" cy="71.799004" data-icsp="MOSI" id="icsp-1-mosi" r="1.9346447" style="stroke-width:0.750153"></circle>
<circle class="pin icsp" cx="186.998" cy="79.157997" data-icsp="RESET" id="icsp-1-reset" r="1.9346447" style="stroke-width:0.750153"></circle>
<circle class="pin icsp" cx="193.44501" cy="79.157997" data-icsp="GND" id="icsp-1-gnd" r="1.9346447" style="stroke-width:0.750153"></circle>
</g><g id="icsp-header-usb" transform="matrix(1.333334,0,0,1.333334,15.459601,0)">
<circle class="pin icsp" cx="65.183998" cy="21.969999" data-icsp="MISO" id="icsp-2-miso" r="1.9346447" style="stroke-width:0.750153"></circle>
<circle class="pin icsp" cx="65.183998" cy="28.417" data-icsp="VCC" id="icsp-2-vcc" r="1.9346447" style="stroke-width:0.750153"></circle>
<circle class="pin icsp" cx="57.824001" cy="21.969999" data-icsp="SCK" id="icsp-2-sck" r="1.9346447" style="stroke-width:0.750153"></circle>
<circle class="pin icsp" cx="57.824001" cy="28.417" data-icsp="MOSI" id="icsp-2-mosi" r="1.9346447" style="stroke-width:0.750153"></circle>
<circle class="pin icsp" cx="50.465" cy="21.969999" data-icsp="RESET" id="icsp-2-reset" r="1.9346447" style="stroke-width:0.750153"></circle>
<circle class="pin icsp" cx="50.465" cy="28.417" data-icsp="GND" id="icsp-2-gnd" r="1.9346447" style="stroke-width:0.750153"></circle>
</g><circle class="led led-orange" cx="131.80901" cy="40.933357" data-linked-pin="13" data-state="off" fill="#ff8c00" id="led-pin13" r="2.5795276" style="stroke-width:1.0002"></circle><circle class="led led-yellow" cx="131.80901" cy="61.857365" data-linked-pin="1" data-state="off" id="led-tx" r="2.5795276" style="stroke-width:1.0002"></circle><circle class="led led-yellow" cx="131.80901" cy="70.905373" data-linked-pin="0" data-state="off" id="led-rx" r="2.5795276" style="stroke-width:1.0002"></circle><circle class="led led-green" cx="246.02504" cy="62.138699" data-state="on" id="led-power" r="2.5795276" style="stroke-width:1.0002"></circle><g class="button" id="reset-button" transform="matrix(1.333334,0,0,1.333334,15.459601,0)">
<rect height="13.817" id="reset-button-body" style="fill:#ebebeb" width="13.817" x="17.174999" y="3.415"></rect>
<circle cx="24.084" cy="10.323" id="reset-button-button" r="4.487" style="fill:#931515"></circle>
</g><g class="connector connector-usb" id="usb-connector" transform="matrix(1.333334,0,0,1.333334,4.1068237,0)">
<circle cx="34.865124" cy="24.698" id="path5" r="3.259841" style="clip-rule:evenodd;fill:#d7dadb;fill-opacity:1;fill-rule:evenodd;stroke:none;stroke-width:0.960811;stroke-linecap:square;stroke-linejoin:round;paint-order:markers fill stroke"></circle><circle cx="34.865124" cy="58.71373" id="circle5" r="3.259841" style="clip-rule:evenodd;fill:#d7dadb;fill-opacity:1;fill-rule:evenodd;stroke:none;stroke-width:0.960811;stroke-linecap:square;stroke-linejoin:round;paint-order:markers fill stroke"></circle><rect height="34.015732" id="usb-connector-body" style="clip-rule:evenodd;fill:#585858;fill-rule:evenodd;stroke-width:1.16228" width="45.354309" x="-3.0801163" y="24.698"></rect></g><g class="connector connector-power" id="power-jack" transform="matrix(1.333334,0,0,1.333334,0.00133375,0)">
<circle cx="13.681" cy="126.589" id="power-jack-outer" r="7.566"></circle>
<circle cx="13.681" cy="126.589" id="power-jack-inner" r="4.3790002" style="fill:#404040"></circle>
<path d="m 7.716,137.213 v 1.094 c 0,0.371 -0.271,0.67 -0.608,0.67 H 0.607 c -0.337,0 -0.608,-0.298 -0.608,-0.67 v -23.725 c 0,-0.371 0.271,-0.67 0.608,-0.67 h 6.501 c 0.337,0 0.608,0.298 0.608,0.67 v 1.092 h 29.665 v 21.539 z" id="power-jack-shell" style="fill:#333333"></path>
</g><rect height="11.893339" id="crystal-16mhz" style="fill:#ffffff;stroke-width:1.33333" width="34.818687" x="79.158302" y="97.818718"></rect><g id="chip-atmega328p" transform="matrix(1.333334,0,0,1.333334,15.459601,0)">
<path d="m 87.854,90.003 h 99.455 c 0.399,0 0.719,0.439 0.719,0.983 v 26.476 c 0,0.544 -0.322,0.983 -0.719,0.983 H 87.854 c -0.399,0 -0.719,-0.439 -0.719,-0.983 V 90.986 c 0,-0.544 0.322,-0.983 0.719,-0.983 z" id="chip-atmega328p-base" style="fill:#1a1a1a;fill-rule:nonzero"></path>
<rect height="20.802999" id="chip-atmega328p-body" style="fill:#2c3034" width="100.776" x="87.194" y="93.821999"></rect>
<path d="m 182.811,114.92 h 2.824 v 1.347 h -2.824 z m -7.316,0 h 4.299 v 1.347 h -4.299 z m -7.316,0 h 4.299 v 1.347 h -4.299 z m -7.316,0 h 4.299 v 1.347 h -4.299 z m -7.316,0 h 4.299 v 1.347 h -4.299 z m -7.316,0 h 4.299 v 1.347 h -4.299 z m -7.316,0 h 4.299 v 1.347 h -4.299 z m -7.316,0 h 4.299 v 1.347 h -4.299 z m -7.316,0 h 4.299 v 1.347 h -4.299 z m -7.316,0 h 4.299 v 1.347 h -4.299 z m -7.316,0 h 4.299 v 1.347 h -4.299 z m -7.316,0 h 4.299 v 1.347 h -4.299 z m -7.316,0 h 4.299 v 1.347 h -4.299 z m -5.734,0 h 3.137 v 1.347 h -3.137 z m 93.287,-22.15 h 3.137 v 1.347 h -3.137 z m -6.94,0 h 4.299 v 1.347 h -4.299 z m -7.459,0 h 4.299 v 1.347 h -4.299 z m -7.209,0 h 4.299 v 1.347 h -4.299 z m -7.417,0 h 4.299 v 1.347 h -4.299 z m -7.252,0 h 4.299 v 1.347 h -4.299 z m -7.48,0 h 4.299 v 1.347 h -4.299 z m -7.231,0 h 4.299 v 1.347 h -4.299 z m -7.231,0 h 4.299 v 1.347 h -4.299 z m -7.231,0 h 4.299 v 1.347 h -4.299 z m -7.424,0 h 4.299 v 1.347 h -4.299 z m -7.424,0 h 4.299 v 1.347 h -4.299 z m -7.258,0 h 4.299 v 1.347 h -4.299 z m -5.95,0 h 2.992 v 1.347 h -2.992 z" id="chip-atmega328p-pins" style="fill:#c7c3c2;fill-rule:nonzero"></path>
<text id="text40" style="font-size:5.867px;font-family:RobotoMono-Regular, 'Roboto Mono', monospace;fill:#ffffff" x="89.608002" y="111.832">ATmega328P</text>
</g><g id="chip-atmega16u2" transform="matrix(1.333334,0,0,1.333334,15.459601,0)">
<path d="m 54.875,47.473 v -0.488 h 0.914 v 0.488 z m 0,1.351 v -0.488 h 0.914 v 0.488 z m 0,1.351 v -0.488 h 0.914 v 0.488 z m 0,1.351 v -0.488 h 0.914 v 0.488 z m 0,1.517 v -0.488 h 0.914 v 0.488 z m 0,1.517 v -0.488 h 0.914 v 0.488 z m 0,1.351 v -0.488 h 0.914 v 0.488 z m 0,1.579 v -0.488 h 0.914 v 0.488 z m 3.101,2.6 h -0.488 v -0.914 h 0.488 z m 1.417,0 h -0.488 v -0.914 h 0.488 z m 1.417,0 h -0.488 v -0.914 h 0.488 z m 1.417,0 h -0.488 v -0.914 h 0.488 z m 1.417,0 h -0.488 v -0.914 h 0.488 z m 1.417,0 h -0.488 v -0.914 h 0.488 z m 1.417,0 H 65.99 v -0.914 h 0.488 z m 1.469,0 h -0.488 v -0.914 h 0.488 z m 2.74,-3.2 v 0.488 H 69.773 V 56.89 Z m 0,-1.417 v 0.488 h -0.914 v -0.488 z m 0,-1.417 v 0.488 h -0.914 v -0.488 z m 0,-1.417 v 0.488 h -0.914 v -0.488 z m 0,-1.417 v 0.488 h -0.914 v -0.488 z m 0,-1.359 v 0.488 h -0.914 v -0.488 z m 0,-1.484 v 0.488 h -0.914 v -0.488 z m 0,-1.366 v 0.488 h -0.914 v -0.488 z m -3.222,-2.828 h 0.488 v 0.914 h -0.488 z m -1.473,0 h 0.488 v 0.914 h -0.488 z m -1.473,0 h 0.488 v 0.914 h -0.488 z m -1.341,0 h 0.488 v 0.914 h -0.488 z m -1.341,0 h 0.488 v 0.914 h -0.488 z m -1.444,0 h 0.488 v 0.914 h -0.488 z m -1.444,0 h 0.488 v 0.914 h -0.488 z m -1.444,0 h 0.488 v 0.914 h -0.488 z" id="chip-atmega16u2-pins" style="fill:#d7dadb"></path>
<rect height="13.991" id="chip-atmega16u2-body" style="fill:#2c3034" width="13.991" x="55.787998" y="45.139"></rect>
<text id="text41" style="font-size:3.52px;font-family:RobotoMono-Regular, 'Roboto Mono', monospace;fill:#ffffff" x="52.290001" y="64.348">ATmega16U2</text>
</g><g id="voltage-regulator" transform="matrix(1.333334,0,0,1.333334,15.459601,0)">
<path d="m 23.464,92.423 h 9.432 v 17.07 h -9.432 z m 3.78,-21.051 h 3.783 v 7.982 h -3.783 z m 38.835,31.906 h 3.783 v 7.982 H 66.079 Z M 56.025,89.093 h 7.98 v 8.024 h -7.98 z" id="voltage-regulator-chips" style="fill:#53555f;fill-rule:nonzero"></path>
<path d="m 32.896,108.401 v -1.755 h 3.828 v 1.755 z m 0,-6.565 v -1.755 h 3.828 v 1.755 z m 0,-6.312 v -1.755 h 3.828 v 1.755 z m -15.068,9.755 v -8.642 h 5.636 v 8.642 z M 31.027,73.198 v -0.987 h 2.021 v 0.987 z m 0,5.377 v -0.987 h 2.021 v 0.987 z m -5.804,-2.821 v -0.987 h 2.021 v 0.987 z m 44.638,29.348 v -0.987 h 2.021 v 0.987 z m 0,5.377 v -0.987 h 2.021 v 0.987 z m -5.804,0 v -0.987 h 2.021 v 0.987 z m 0,-2.821 v -0.987 h 2.021 v 0.987 z m 0,-2.557 v -0.987 h 2.021 v 0.987 z m -0.053,-9.012 v -0.987 h 1.786 v 0.987 z m 0,-1.818 v -0.987 h 1.786 v 0.987 z m 0,-1.818 v -0.987 h 1.786 v 0.987 z m 0,-1.818 v -0.987 h 1.786 v 0.987 z M 54.238,96.1 v -0.987 h 1.786 V 96.1 Z m 0,-1.839 v -0.987 h 1.786 v 0.987 z m 0,-1.839 v -0.987 h 1.786 v 0.987 z m 0,-1.839 v -0.987 h 1.786 v 0.987 z" id="voltage-regulator-pins" style="fill:#d7dadb;fill-rule:nonzero"></path>
</g>
</g>
<!-- ============================================== -->
<!-- EXTERNAL BREADBOARD / OFF-BOARD COMPONENTS     -->
<!-- ============================================== -->
<!-- 1. MODULAR 5mm LED WITH INTEGRATED RESISTOR (LED_ongrid.svg) -->
<g id="led-5mm" transform="translate(680, 100) scale(0.95)">
<g id="leadframe">
<path class="led-cls-1" d="M144 36 L144 108 C144 127.869 127.869 144 108 144 L36 144 C16.131 144 0 127.869 0 108 L0 36 C0 16.131 16.131 0 36 0 L108 0 C127.869 0 144 16.131 144 36 Z" id="path1"></path>
<path class="led-cls-2" d="M141.841 37.079 L141.841 106.918 C141.841 126.191 126.194 141.838 106.921 141.838 L37.082 141.838 C17.809 141.838 2.162 126.191 2.162 106.918 L2.162 37.079 C2.162 17.806 17.809 2.159 37.082 2.159 L106.921 2.159 C126.194 2.159 141.841 17.806 141.841 37.079 Z" id="path2"></path>
</g>
<g id="case">
<circle class="led-cls-1" cx="72.001" cy="71.999" id="frame" r="48.107"></circle>
<circle class="led-cls-3 on" cx="72.001" cy="71.999" data-component-id="led" data-led-type="standard-thru-hole" data-param-color="var(--df-color)" data-param-if-max="var(--df-if-max)" data-param-if-rated="var(--df-if-rated)" data-param-luminous-intensity="var(--df-luminous-intensity)" data-param-power-rated="var(--df-power-rated)" data-param-vf="var(--df-vf)" data-param-vr-max="var(--df-vr-max)" data-param-wavelength-peak="var(--df-wavelength-peak)" id="led-body" r="45.22"></circle>
<ellipse class="led-cls-4" cx="73.504" cy="69.995" id="lens" rx="42.333" ry="41.372"></ellipse>
<path class="led-cls-5" d="M92.04 38.236 C96.884 41.367 97.74 42.698 99.098 43.798 C99.508 44.13 100.955 45.896 101.875 46.828 C102.231 47.188 103.384 48.889 103.51 49.073 C104.283 50.214 105.336 51.406 106.831 54.687 C107.511 56.175 107.967 57.092 108.07 57.299 C108.14 57.439 108.068 57.465 108.773 59.127 C109.078 59.844 108.457 59.529 106.888 60.168 C104.888 60.981 104.839 60.797 102.83 61.597 C101.442 62.149 101.214 61.76 101.146 61.644 C100.805 61.066 99.591 56.923 96.211 52.644 C92.591 48.061 91.113 47.519 89.661 46.26 C87.885 44.718 82.983 42.49 82.375 42.214 C81.738 41.924 81.331 41.875 81.825 40.966 C81.962 40.714 82.199 39.761 82.367 39.379 C82.558 38.946 82.623 38.993 82.812 38.555 C82.851 38.465 82.853 38.135 82.957 37.9 C84.233 35.01 83.992 34.383 84.757 34.7 C85.495 35.007 85.525 34.902 86.251 35.254 C88.229 36.209 88.312 36.026 92.038 38.239 Z" id="reflex"></path>
</g>
<g id="wire-bond">
<g id="anodes">
<circle class="df-pin power led-cls-3" cx="116" cy="124.107" data-pin="A" data-port-direction="input" data-port-type="power" id="A2" r="4"></circle>
<circle class="df-pin power led-cls-3" cx="20" cy="20.107" data-pin="A" data-port-direction="input" data-port-type="power" id="A1" r="4"></circle>
</g>
<g id="cathodes">
<circle class="df-pin ground led-cls-3" cx="124" cy="20.107" data-pin="K" data-port-direction="input" data-port-type="ground" id="K2" r="4"></circle>
<circle class="df-pin ground led-cls-3" cx="20" cy="124.107" data-pin="K" data-port-direction="input" data-port-type="ground" id="K1" r="4"></circle>
</g>
<g>
<rect class="led-cls-6" height="8" width="21.668" x="61.166" y="8.107"></rect>
<rect class="led-cls-7" height="8" width="16" x="64" y="8.107"></rect>
<text class="led-cls-8" transform="matrix(0.429 0 0 0.497 64 14.073)" x="0" xml:space="preserve" y="0"><tspan x="0" y="0">RSMDΩ</tspan></text>
</g>
</g>
</g>
<!-- 2. TACTILE PUSHBUTTON NO (Button_NO.svg) -->
<g aria-label="Pushbutton Switch" class="cursor-pointer" id="tactile-switch" role="button" tabindex="0" transform="translate(680, 290) scale(0.95)">
<g id="cmd-pushbutton-no-start">
<g>
<path class="housing btn-cls-1" d="M11.676 0 L132.324 0 C138.773 0 144 5.227 144 11.676 L144 132.324 C144 138.773 138.773 144 132.324 144 L11.676 144 C5.227 144 0 138.773 0 132.324 L0 11.676 C0 5.227 5.227 0 11.676 0 Z"></path>
</g>
<circle class="bezel btn-cls-2" cx="72" cy="72" r="48"></circle>
<g id="actuator" transform="translate(0, 2.5)">
<g>
<circle class="btn-cls-3" cx="72" cy="72" r="40"></circle>
</g>
</g>
<g class="open" id="contact">
<polyline class="btn-cls-4" id="contact-arm" points="48,136 64.696,136 75.13,127.652"></polyline>
<line class="btn-cls-4" x1="79.304" x2="96" y1="136" y2="136"></line>
</g>
<text class="btn-cls-5" id="port-NO1" transform="matrix(1.8 0 0 1.8 72 75.72)" x="0" xml:space="preserve" y="0"><tspan x="0" y="0">NO</tspan></text>
<circle class="btn-cls-6" cx="132" cy="132" data-name="Connector 2b" id="Connector_2b" r="4"></circle>
<circle class="btn-cls-6" cx="12" cy="132" data-name="Connector 2a" id="Connector_2a" r="4"></circle>
<circle class="btn-cls-6" cx="132" cy="12" data-name="Connector 1b" id="Connector_1b" r="4"></circle>
<circle class="btn-cls-6" cx="12" cy="12" data-name="Connector 1a" id="Connector_1a" r="4"></circle>
</g>
</g>
<!-- MCU pull-up callout (no discrete pull resistor on canvas) -->
<g id="mcu-pullup-note" transform="translate(360, 168)">
<rect fill="#0f172a" fill-opacity="0.85" height="28" rx="4" stroke="#34d399" stroke-width="1" width="168" x="0" y="0"></rect>
<text fill="#6ee7b7" font-family="'JetBrains Mono', monospace" font-size="9" x="8" y="12">D2 · MCU pull-up</text>
<text fill="#94a3b8" font-family="'JetBrains Mono', monospace" font-size="8" x="8" y="22">pressed → active-LOW</text>
</g>
<!-- ============================================== -->
<!-- REALISTIC BENT JUMPER WIRES (SVG BEZIER PATHS) -->
<!-- Button: D2 ↔ GND with MCU INPUT_PULLUP         -->
<!-- LED: D13 ↔ GND, integrated SMD resistance      -->
<!-- ============================================== -->
<g filter="url(#shadowGlow)" id="circuit-wiring" stroke-linecap="round" stroke-linejoin="round">
<!-- 1. WIRE GREEN: Arduino Digital Pin 2 → Button NO (Connector_2a) -->
<path d="M 355.9 147.8 C 355.9 80, 620 70, 650 240 C 665 320, 660 415.4, 691.4 415.4" fill="none" stroke="#22c55e" stroke-width="4.5"></path>
<circle cx="355.9" cy="147.8" fill="#14532d" r="4" stroke="#86efac" stroke-width="1.5"></circle>
<circle cx="691.4" cy="415.4" fill="#14532d" r="3.5" stroke="#86efac" stroke-width="1.5"></circle>
<!-- 2. WIRE BLACK: Button COM (Connector_1a) → Arduino Power GND -->
<path d="M 691.4 301.4 C 640 360, 420 430, 266.8 385.8" fill="none" stroke="#475569" stroke-width="4.5"></path>
<circle cx="691.4" cy="301.4" fill="#1e293b" r="3.5" stroke="#94a3b8" stroke-width="1.5"></circle>
<circle cx="266.8" cy="385.8" fill="#1e293b" r="4" stroke="#94a3b8" stroke-width="1.5"></circle>
<!-- 3. WIRE SKY-BLUE: Pin 13 to External LED Anode A1 -->
<path d="M 218.8 147.8 C 218.8 45, 610 35, 699.0 119.1" fill="none" stroke="#38bdf8" stroke-width="4"></path>
<circle cx="218.8" cy="147.8" fill="#0369a1" r="3.5" stroke="#bae6fd" stroke-width="1.5"></circle>
<circle cx="699.0" cy="119.1" fill="#0369a1" r="3.5" stroke="#bae6fd" stroke-width="1.5"></circle>
<!-- 4. WIRE DASHED SLATE: LED Cathode K1 to Digital Header GND -->
<path d="M 699.0 217.9 C 630 220, 480 210, 360 185 C 270 165, 207.2 165, 207.2 147.8" fill="none" stroke="#64748b" stroke-dasharray="6,4" stroke-width="3.5"></path>
<circle cx="699.0" cy="217.9" fill="#1e293b" r="3.5" stroke="#94a3b8" stroke-width="1.5"></circle>
<circle cx="207.2" cy="147.8" fill="#1e293b" r="3.5" stroke="#94a3b8" stroke-width="1.5"></circle>
</g>
<!-- Current Flow Animated Particle Annotations (WCAG Safe, High Contrast) -->
<g>
<circle cx="520" cy="360" fill="#86efac" r="2.5">
<animate attributename="opacity" dur="1.2s" repeatcount="indefinite" values="0.3;1;0.3"></animate>
</circle>
<circle cx="460" cy="52" fill="#7dd3fc" r="2.5">
<animate attributename="opacity" dur="0.9s" repeatcount="indefinite" values="1;0.2;1"></animate>
</circle>
</g>
</svg>"####;
const CPP: &str = "#include <Arduino.h>\n\nconst int btnPin = 2;\nconst int ledPin = 13;\nbool pressed = false;\n\nvoid setup() {\n  pinMode(btnPin, INPUT_PULLUP);\n  pinMode(ledPin, OUTPUT);\n  digitalWrite(ledPin, LOW);\n}\n\nvoid loop() {\n  pressed = !digitalRead(btnPin);\n  digitalWrite(ledPin, pressed);\n}\n";
const ST: &str = "PROGRAM MAIN\nVAR\n    pressed : BOOL := FALSE;\nEND_VAR\n\nSETUP\n    CFG_DIGITAL_INPUT(btnPin);\n    CFG_DIGITAL_OUTPUT(ledPin);\n    SET_BOOL(ledPin, FALSE);\n\nLOOP\n    pressed := NOT GET_BOOL(btnPin);\n    SET_BOOL(ledPin, pressed);\nEND_PROGRAM\n";
const PY: &str = "from machine import Pin\n\nbtn = Pin(2, Pin.IN, Pin.PULL_UP)\nled = Pin(13, Pin.OUT, value=0)\n\nwhile True:\n    pressed = not btn.value()\n    led.value(pressed)\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View { Simulation, Code, Ladder, Grafcet }
impl View {
    const ALL: [Self; 4] = [Self::Simulation, Self::Code, Self::Ladder, Self::Grafcet];
    fn label(self) -> &'static str { match self {
        Self::Simulation => "Simulation", Self::Code => "Code",
        Self::Ladder => "Ladder", Self::Grafcet => "GRAFCET",
    }}
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source { Cpp, St, Python, Toml }
impl Source {
    const ALL: [Self; 4] = [Self::Cpp, Self::St, Self::Python, Self::Toml];
    fn label(self) -> &'static str { match self {
        Self::Cpp => "arduino/main.cpp", Self::St => "st/main.st",
        Self::Python => "micropython/main.py", Self::Toml => "project.toml",
    }}
    fn content(self) -> &'static str { match self {
        Self::Cpp => CPP, Self::St => ST, Self::Python => PY, Self::Toml => PROJECT,
    }}
}
#[derive(Debug, Clone)]
enum Message {
    View(View), Source(Source), Run, Step, Reset, Tick,
    Raw(bool), Physical(bool), ToggleRaw, Left, Right, Bottom, Zoom(f32), Copy,
}
struct App {
    view: View, source: Source, running: bool, cycles: u64,
    raw: bool, pressed: bool, led: bool,
    left: bool, right: bool, bottom: bool, zoom: f32,
    off: svg::Handle, on: svg::Handle,
    canvas: Option<SimCanvas>,
    project_path: PathBuf,
    status: String,
}
impl App {
    fn boot() -> (Self, iced::Task<Message>) {
        let project_path = c01_path();
        let mut app = Self {
            view: View::Simulation, source: Source::Cpp, running: true, cycles: 0,
            raw: true, pressed: false, led: false,
            left: true, right: true, bottom: true, zoom: 1.0,
            off: svg::Handle::from_memory(SCHEMATIC_OFF.as_bytes()),
            on: svg::Handle::from_memory(SCHEMATIC_ON.as_bytes()),
            canvas: None,
            project_path: project_path.clone(),
            status: String::new(),
        };
        app.reboot_sim();
        (app, iced::Task::none())
    }

    fn reboot_sim(&mut self) {
        let root = repo_root();
        match load_project(&self.project_path) {
            Ok(project) => match SimCanvas::from_project(&project, &root, &SimConfig::default()) {
                Ok(mut canvas) => match canvas.boot() {
                    Ok(_frame) => {
                        // Startup smoke: D2 HIGH, pressed FALSE, D13 FALSE
                        self.raw = canvas.sim.pin(canvas.binding.button_pin);
                        self.pressed = false;
                        self.led = false;
                        self.cycles = 0;
                        self.canvas = Some(canvas);
                        self.status = format!(
                            "loaded {} · dendri-sim + prototype chrome",
                            self.project_path.display()
                        );
                    }
                    Err(e) => {
                        self.canvas = None;
                        self.status = format!("sim boot failed: {e}");
                    }
                },
                Err(e) => {
                    self.canvas = None;
                    self.status = format!("canvas load failed: {e}");
                }
            },
            Err(e) => {
                self.canvas = None;
                self.status = format!("project load failed: {e}");
            }
        }
    }

    fn scan(&mut self) {
        let Some(canvas) = self.canvas.as_mut() else {
            // Prototype fallback when sim assets missing
            self.pressed = !self.raw;
            self.led = self.pressed;
            self.cycles = self.cycles.saturating_add(1);
            return;
        };
        match canvas.step() {
            Ok(frame) => {
                let raw = canvas.sim.pin(canvas.binding.button_pin);
                let pressed = match canvas.sim.var("pressed") {
                    Some(dendri_sim::Value::Bool(v)) => *v,
                    _ => frame.button_pressed,
                };
                self.cycles = frame.cycle;
                self.led = frame.led_on;
                self.raw = raw;
                self.pressed = pressed;
                self.status = format!(
                    "scan {} · btnPin={} pressed={} ledPin={}",
                    self.cycles,
                    bit(self.raw),
                    bit(self.pressed),
                    bit(self.led)
                );
            }
            Err(e) => self.status = format!("scan error: {e}"),
        }
    }

    fn apply_physical(&mut self, down: bool) {
        self.raw = !down;
        if let Some(canvas) = self.canvas.as_mut() {
            canvas.set_button(down);
            self.raw = canvas.sim.pin(canvas.binding.button_pin);
        }
        if self.running {
            self.scan();
        }
    }

    fn apply_raw(&mut self, raw: bool) {
        self.raw = raw;
        if let Some(canvas) = self.canvas.as_mut() {
            // raw HIGH = released for pull-up → physical pressed = false
            let pressed = if canvas.binding.button_active_low {
                !raw
            } else {
                raw
            };
            canvas.set_button(pressed);
            self.raw = canvas.sim.pin(canvas.binding.button_pin);
        }
        if self.running {
            self.scan();
        }
    }

    fn update(&mut self, message: Message) -> iced::Task<Message> {
        match message {
            Message::View(view) => self.view = view,
            Message::Source(source) => self.source = source,
            Message::Run => self.running = !self.running,
            Message::Step => { self.running = false; self.scan(); }
            Message::Reset => {
                let was_running = self.running;
                self.reboot_sim();
                self.running = was_running;
            }
            Message::Tick => if self.running { self.scan(); },
            Message::Raw(raw) => self.apply_raw(raw),
            Message::Physical(down) => self.apply_physical(down),
            Message::ToggleRaw => self.apply_raw(!self.raw),
            Message::Left => self.left = !self.left,
            Message::Right => self.right = !self.right,
            Message::Bottom => self.bottom = !self.bottom,
            Message::Zoom(delta) => self.zoom = (self.zoom + delta).clamp(0.7, 1.5),
            Message::Copy => return iced::clipboard::write(self.source.content().to_owned()),
        }
        iced::Task::none()
    }
    fn subscription(&self) -> Subscription<Message> {
        if self.running { iced::time::every(Duration::from_millis(100)).map(|_| Message::Tick) }
        else { Subscription::none() }
    }
    fn view(&self) -> Element<'_, Message> {
        let mut tabs = row![].spacing(4);
        for v in View::ALL { tabs = tabs.push(button(text(v.label()).size(12))
            .padding([7, 10]).style(if self.view == v { ui::selected } else { ui::control })
            .on_press(Message::View(v))); }
        let header = row![
            text("Δ").size(25).color(ui::ORANGE),
            text("DendriForge").size(21).color(ui::HEADING), text("v0.6.0").size(11).color(ui::MUTED),
            Space::with_width(12), text("PROJECTS / UNO R3 / C01-button-led").size(11).color(ui::SKY),
            Space::with_width(Length::Fill), tabs,
        ].spacing(9).align_y(alignment::Vertical::Center);
        let tools = row![
            action(if self.running { "PAUSE" } else { "RUN" }, Message::Run),
            action("STEP CYCLE", Message::Step), action("RESET", Message::Reset),
            text(if self.running { "● PREVIEW RUNNING" } else { "● PREVIEW PAUSED" })
                .size(12).color(if self.running { ui::EMERALD } else { ui::ORANGE }),
            text(format!("Cycle: {}", self.cycles)).size(12).color(ui::CYAN),
            Space::with_width(Length::Fill),
            action(if self.left { "Hide left" } else { "Show left" }, Message::Left),
            action(if self.right { "Hide right" } else { "Show right" }, Message::Right),
            action(if self.bottom { "Hide dock" } else { "Show dock" }, Message::Bottom),
            action("−", Message::Zoom(-0.1)),
            text(format!("{:.0}%", self.zoom * 100.0)).size(12),
            action("+", Message::Zoom(0.1)),
        ].spacing(8).align_y(alignment::Vertical::Center);
        let mut workspace = row![].spacing(1).height(Length::Fill);
        if self.left { workspace = workspace.push(container(scrollable(self.palette()))
            .padding(12).width(215).height(Length::Fill).style(ui::panel)); }
        let center = match self.view {
            View::Simulation => self.simulation(), View::Code => self.code(),
            View::Ladder | View::Grafcet => self.diagram(),
        };
        workspace = workspace.push(container(center).width(Length::Fill).height(Length::Fill)
            .style(|_| ui::surface(ui::SURFACE)));
        if self.right { workspace = workspace.push(container(scrollable(self.inspector()))
            .padding(12).width(310).height(Length::Fill).style(ui::panel)); }
        let mut content = column![
            container(header).padding([9,12]).width(Length::Fill).style(ui::panel),
            container(tools).padding([8,12]).width(Length::Fill).style(|_| ui::surface(ui::LOWEST)),
            workspace,
        ].spacing(1);
        if self.bottom {
            content = content.push(container(row![
                column![caption("LOCAL PREVIEW CONSOLE"),
                    text(format!("[{}] btnPin={}   pressed={}   ledPin={}", self.cycles,
                        bit(self.raw), bit(self.pressed), bit(self.led))).font(Font::MONOSPACE).size(12),
                    text(if self.status.is_empty() {
                        "100 ms UI tick · dendri-sim scan · no serial device connected".into()
                    } else {
                        format!("100 ms UI tick · {}", self.status)
                    }).size(11).color(ui::MUTED)
                ].spacing(9).width(Length::Fill),
                column![caption("BUILD / MEMORY"),
                    text("Flash: not compiled   SRAM: not measured").size(12),
                    text("ASL sample · local state shared by all four views").size(11).color(ui::MUTED)
                ].spacing(9).width(Length::Fill),
            ].spacing(20)).padding(12).width(Length::Fill).style(ui::panel));
        }
        content = content.push(container(row![
            text("AVR ATmega328P · 16 MHz target · LOCAL PREVIEW").size(11).color(ui::EMERALD),
            Space::with_width(Length::Fill),
            text("D2 INPUT_PULLUP · D13 OUTPUT · integrated 220 Ω").size(11).color(ui::MUTED)
        ]).padding([5,12]).width(Length::Fill).style(ui::panel));
        container(content).height(Length::Fill).style(|_| ui::surface(ui::MAIN)).into()
    }
    fn palette(&self) -> Element<'_, Message> {
        let title = match self.view { View::Simulation => "COMPONENT CATALOG", View::Code => "PROJECT EXPLORER",
            View::Ladder => "INSTRUCTIONS · LD", View::Grafcet => "PALETTE · GRAFCET" };
        let mut items = column![caption(title)].spacing(12);
        match self.view {
            View::Code => for source in Source::ALL {
                items = items.push(action(source.label(), Message::Source(source)));
            },
            View::Simulation => for item in ["Uno R3 · ATmega328P", "led1 · Red LED module", "Integrated resistor · 220 Ω", "btn_start · NO pushbutton"] {
                items = items.push(text(item).size(12));
            },
            View::Ladder => for item in ["CONTACTS", "| |  Normally open", "|/|  Normally closed", "|P|  Rising edge", "|N|  Falling edge", "COILS", "( )  Output", "(S)  Set   (R) Reset", "TIMERS & COUNTERS", "TON  TOF  TP", "CTU  CTD  CTUD", "MATH & COMPARE", "EQU  NEQ  GRT  LES"] {
                items = items.push(text(item).size(12).color(ui::MUTED));
            },
            View::Grafcet => for item in ["STEPS & NODES", "▣ Initial step", "□ Standard step", "M Macro-step", "TRANSITIONS & LINKS", "Transition bar", "OR branch · AND sync", "Loopback wire", "ACTION BLOCK", "Ordered cyclic statements", "No transitions in this sample"] {
                items = items.push(text(item).size(12).color(ui::MUTED));
            },
        }
        items = items.push(Space::with_height(20)).push(caption("ASL SYMBOLS"))
            .push(text("D2    btnPin · DI").font(Font::MONOSPACE).size(12).color(ui::CYAN))
            .push(text("VAR   pressed · BOOL").font(Font::MONOSPACE).size(12).color(ui::SECONDARY))
            .push(text("D13   ledPin · DO").font(Font::MONOSPACE).size(12).color(ui::ORANGE))
            .push(text("Palette reference only.\nAuthoring tools are not connected.").size(11).color(ui::MUTED));
        items.into()
    }
    fn simulation(&self) -> Element<'_, Message> {
        let handle = if self.led { self.on.clone() } else { self.off.clone() };
        let picture = widget::mouse_area(svg(handle).width(Length::Fill).height(Length::Fill)
            .content_fit(iced::ContentFit::Contain))
            .on_press(Message::Physical(true)).on_release(Message::Physical(false))
            .on_exit(Message::Physical(false));
        column![container(row![caption("CIRCUIT SIMULATION"), Space::with_width(Length::Fill),
            text("Uno + modular LED + NO button").size(12).color(ui::MUTED)])
            .padding(14),
            picture,
            container(checkbox("Hold button (active-LOW)", !self.raw).on_toggle(Message::Physical))
                .padding(14),
            container(text("Hold the schematic with the mouse, or use the accessible button control below it. The whole schematic is the mouse hit area.")
                .size(11).color(ui::MUTED)).padding([0,14]),
        ].spacing(8).height(Length::Fill).into()
    }
    fn code(&self) -> Element<'_, Message> {
        let mut tabs = row![].spacing(4);
        for source in Source::ALL { tabs = tabs.push(button(text(source.label()).size(11))
            .padding(8).on_press(Message::Source(source))
            .style(if source == self.source { ui::selected } else { ui::control })); }
        let mut lines = column![].spacing(3);
        for (index, line) in self.source.content().lines().enumerate() {
            lines = lines.push(row![text(format!("{:>3}", index + 1)).font(Font::MONOSPACE)
                .size(12).color(ui::MUTED).width(40),
                text(line.to_owned()).font(Font::MONOSPACE).size(12).color(syntax(line))].spacing(12));
        }
        column![container(row![tabs, Space::with_width(Length::Fill), action("Copy", Message::Copy)])
                .padding(8).style(ui::panel),
            container(scrollable(lines)).padding(16).width(Length::Fill).height(Length::Fill).style(ui::editor),
            container(text("Read-only sample projections · ST uses the DendriForge dialect · MicroPython is not an Uno firmware target")
                .size(11).color(ui::MUTED)).padding(10),
        ].height(Length::Fill).into()
    }
    fn diagram(&self) -> Element<'_, Message> {
        let title = if self.view == View::Ladder { "C01-button-led.LD · TWO ORDERED RUNGS" }
            else { "C01-button-led.GRAFCET · S0 / A_LOOP" };
        column![container(caption(title)).padding(14),
            canvas(Diagram { view: self.view, raw: self.raw, pressed: self.pressed,
                led: self.led, zoom: self.zoom }).width(Length::Fill).height(Length::Fill),
            container(checkbox("btnPin / D2 = HIGH (released)", self.raw).on_toggle(Message::Raw)).padding(14),
            container(text(if self.view == View::Ladder {
                "Rung 1: NOT GET_BOOL(btnPin) → pressed. Rung 2: pressed → ledPin. Click the first contact to toggle D2."
            } else { "S0 remains active. A_LOOP executes the two statements in order once per preview cycle. No transition or self-loop." })
                .size(11).color(ui::MUTED)).padding(14),
        ].height(Length::Fill).into()
    }
    fn inspector(&self) -> Element<'_, Message> {
        let watch = column![caption("LIVE TAG WATCH"),
            tag("btnPin", "D2 / DI", self.raw, ui::CYAN),
            tag("pressed", "BOOL", self.pressed, ui::SECONDARY),
            tag("ledPin", "D13 / DO", self.led, ui::ORANGE),
        ].spacing(12);
        column![caption("INSPECTOR · C01-button-led"),
            container(watch).padding(12).style(ui::card),
            checkbox("Physical button pressed", !self.raw).on_toggle(Message::Physical),
            container(column![caption("HARDWARE CONFIGURATION"),
                text("btnPin · INPUT_PULLUP\nReleased = HIGH\nPressed = LOW\n\nledPin · OUTPUT\nLED resistor: integrated 220 Ω").size(12),
            ].spacing(12)).padding(12).style(ui::card),
            container(column![caption("SETUP · ONCE"),
                text("CFG_DIGITAL_INPUT(btnPin)\nCFG_DIGITAL_OUTPUT(ledPin)\nSET_BOOL(ledPin, FALSE)").font(Font::MONOSPACE).size(11),
            ].spacing(12)).padding(12).style(ui::card),
            container(column![caption("ASL LOOP · ORDERED"),
                text("pressed := NOT\n  GET_BOOL(btnPin);\nSET_BOOL(ledPin, pressed);").font(Font::MONOSPACE).size(12).color(ui::SECONDARY),
            ].spacing(12)).padding(12).style(ui::card),
            text("Prototype chrome + dendri-sim. No general ASL authoring, compiler, uploader, or electrical solver is connected.")
                .size(11).color(ui::MUTED),
        ].spacing(16).into()
    }
}
fn bit(value: bool) -> &'static str { if value { "TRUE" } else { "FALSE" } }
fn caption<'a>(label: &'a str) -> iced::widget::Text<'a> { text(label).size(12).color(ui::SKY) }
fn action<'a>(label: &'a str, message: Message) -> iced::widget::Button<'a, Message> {
    button(text(label).size(11)).padding([6,9]).style(ui::control).on_press(message)
}
fn tag<'a>(name: &'a str, address: &'a str, value: bool, color: Color) -> Element<'a, Message> {
    row![text(name).font(Font::MONOSPACE).size(12).color(color).width(85),
        text(address).size(11).color(ui::MUTED).width(75),
        text(bit(value)).size(12).color(if value {ui::EMERALD} else {ui::MUTED})].into()
}
fn syntax(line: &str) -> Color {
    let line = line.trim_start();
    if line.starts_with('[') { ui::SECONDARY }
    else if line.starts_with("//") || line.starts_with("# ") { ui::MUTED }
    else if line.contains("digital") || line.contains("GET_BOOL") || line.contains("SET_BOOL") { ui::CYAN }
    else if line.contains("false") || line.contains("FALSE") || line.contains("true") { ui::ORANGE }
    else { ui::TEXT }
}

struct Diagram { view: View, raw: bool, pressed: bool, led: bool, zoom: f32 }
impl canvas::Program<Message> for Diagram {
    type State = ();
    fn update(&self, _: &mut (), event: canvas::Event, bounds: Rectangle, cursor: mouse::Cursor)
        -> (canvas::event::Status, Option<Message>) {
        if self.view == View::Ladder {
            if let canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event {
                if let Some(p) = cursor.position_in(bounds) {
                    let scale = (bounds.width / 800.0).min(bounds.height / 470.0) * self.zoom;
                    if Rectangle::new(Point::new(80.0*scale, 60.0*scale), Size::new(120.0*scale,100.0*scale)).contains(p) {
                        return (canvas::event::Status::Captured, Some(Message::ToggleRaw));
                    }
                }
            }
        }
        (canvas::event::Status::Ignored, None)
    }
    fn draw(&self, _: &(), renderer: &Renderer, _: &Theme, bounds: Rectangle,
        _: mouse::Cursor) -> Vec<canvas::Geometry> {
        let mut f = canvas::Frame::new(renderer, bounds.size());
        f.fill_rectangle(Point::ORIGIN, bounds.size(), ui::SURFACE);
        for x in (0..bounds.width as usize).step_by(20) {
            for y in (0..bounds.height as usize).step_by(20) {
                f.fill(&canvas::Path::circle(Point::new(x as f32,y as f32),0.7),ui::HOVER);
            }
        }
        f.scale((bounds.width / 800.0).min(bounds.height / 470.0) * self.zoom);
        if self.view == View::Ladder {
            line(&mut f, 24.0, 52.0, 24.0, 400.0, ui::EMERALD, 5.0);
            line(&mut f, 775.0, 52.0, 775.0, 400.0, ui::BORDER, 3.0);
            for (index, y, input, output, value) in [
                (1,120.0,"btnPin · D2","pressed",self.pressed),
                (2,310.0,"pressed","ledPin · D13",self.led),
            ] {
                let c = if value {ui::EMERALD} else {ui::BORDER};
                label(&mut f, 42.0,y-63.0,&format!("RUNG {index:03}"),ui::SKY,14.0);
                label(&mut f, 103.0,y-40.0,input,if index==1 {ui::CYAN} else {ui::SECONDARY},14.0);
                line(&mut f,24.0,y,120.0,y,ui::EMERALD,3.0);
                line(&mut f,120.0,y-18.0,120.0,y+18.0,c,3.0);
                line(&mut f,149.0,y-18.0,149.0,y+18.0,c,3.0);
                if index==1 { line(&mut f,113.0,y+24.0,156.0,y-24.0,ui::ROSE,3.0); }
                line(&mut f,149.0,y,676.0,y,c,3.0);
                let coil = canvas::Path::new(|b| {
                    b.move_to(Point::new(693.0,y-21.0));
                    b.bezier_curve_to(Point::new(670.0,y-13.0),Point::new(670.0,y+13.0),Point::new(693.0,y+21.0));
                    b.move_to(Point::new(712.0,y-21.0));
                    b.bezier_curve_to(Point::new(735.0,y-13.0),Point::new(735.0,y+13.0),Point::new(712.0,y+21.0));
                });
                f.stroke(&coil,canvas::Stroke::default().with_color(c).with_width(3.0));
                line(&mut f,728.0,y,775.0,y,c,3.0);
                label(&mut f,632.0,y-40.0,output,if index==1 {ui::SECONDARY} else {ui::ORANGE},14.0);
                label(&mut f,45.0,y+48.0,if index==1 {"pressed := NOT GET_BOOL(btnPin)"} else {"SET_BOOL(ledPin, pressed)"},ui::MUTED,13.0);
            }
            label(&mut f,40.0,430.0,&format!("Raw D2 = {} · pressed = {}",bit(self.raw),bit(self.pressed)),ui::MUTED,13.0);
        } else {
            label(&mut f,50.0,60.0,"1 INITIAL STEP · 0 TRANSITIONS",ui::SKY,15.0);
            for (x,y,w,h) in [(50.0,140.0,98.0,98.0),(57.0,147.0,84.0,84.0)] {
                f.stroke(&canvas::Path::rectangle(Point::new(x,y),Size::new(w,h)),canvas::Stroke::default().with_color(ui::EMERALD).with_width(3.0));
            }
            label(&mut f,78.0,172.0,"S0",ui::EMERALD,28.0);
            label(&mut f,65.0,260.0,"ACTIVE",ui::EMERALD,14.0);
            line(&mut f,148.0,189.0,205.0,189.0,ui::EMERALD,3.0);
            f.fill_rectangle(Point::new(205.0,126.0),Size::new(550.0,160.0),ui::CARD);
            label(&mut f,225.0,144.0,"A_LOOP · ORDERED CYCLIC ACTION",ui::SKY,16.0);
            label(&mut f,225.0,194.0,"1. pressed := NOT GET_BOOL(btnPin);",ui::SECONDARY,16.0);
            label(&mut f,225.0,239.0,"2. SET_BOOL(ledPin, pressed);",ui::ORANGE,16.0);
            label(&mut f,50.0,340.0,"S0 remains active. No self-loop or transition.",ui::MUTED,15.0);
            label(&mut f,50.0,373.0,"Setup runs once before cyclic execution.",ui::MUTED,14.0);
        }
        vec![f.into_geometry()]
    }
}
fn line(f: &mut canvas::Frame, x1:f32,y1:f32,x2:f32,y2:f32,c:Color,width:f32) {
    f.stroke(&canvas::Path::line(Point::new(x1,y1),Point::new(x2,y2)),
        canvas::Stroke::default().with_color(c).with_width(width));
}
fn label(f: &mut canvas::Frame,x:f32,y:f32,s:&str,color:Color,size:f32) {
    f.fill_text(canvas::Text { content:s.to_owned(),position:Point::new(x,y),color,
        size:size.into(),font:Font::MONOSPACE,..Default::default() });
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn c01_path() -> PathBuf {
    repo_root().join("content/fixtures/canvas/C01-button-led.toml")
}

fn load_project(path: &PathBuf) -> Result<dendri_ast::SimulationProject, String> {
    match parse_path(path).map_err(|e| e.to_string())? {
        ParsedDocument::Project(p) => Ok(p.project),
        _ => Err("expected project document".into()),
    }
}

fn main() -> iced::Result {
    iced::application("DendriForge — C01-button-led", App::update, App::view)
        .theme(|_| ui::slate())
        .subscription(App::subscription)
        .window_size((1520.0, 960.0))
        .run_with(App::boot)
}
