// DendriForge Rust frontend — P2 Dendri HAL subset
// project: S04-pwm-adc-map

// ASL_HW: pin=34 type=Analog mode=Di label=potPin role=analog-in
const potPin: u32 = 34;
// ASL_HW: pin=25 type=Digital mode=Do label=fanPwm role=pwm-out
const fanPwm: u32 = 25;

// ASL_DATA: name=raw type=UINT scope=VAR unit=-
let mut raw: u32 = 0;
// ASL_DATA: name=duty type=UINT scope=VAR unit=-
let mut duty: u32 = 0;

fn setup() {
  CFG_ADC_INPUT(potPin);
  CFG_PWM_OUTPUT(fanPwm, 5000);
  analog_write(fanPwm, 0);
}

fn loop_() {
  raw = analog_read(potPin);
  duty = REAL_TO_INT(MAP_RANGE(raw, 0, 65535, 0, 255));
  analog_write(fanPwm, duty);
}
