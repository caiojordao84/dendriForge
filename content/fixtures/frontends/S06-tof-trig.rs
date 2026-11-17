// DendriForge Rust frontend — P2 Dendri HAL subset
// project: S06-iec-tof-trig

// ASL_HW: pin=2 type=Digital mode=Di label=btnPin role=digital-in
const btnPin: u32 = 2;
// ASL_HW: pin=13 type=Digital mode=Do label=ledPin role=digital-out
const ledPin: u32 = 13;

// ASL_DATA: name=PT type=TIME scope=CONST unit=ms
const PT: u32 = 200;
// ASL_DATA: name=STEP type=TIME scope=CONST unit=ms
const STEP: u32 = 100;
// ASL_DATA: name=btn type=BOOL scope=VAR unit=-
let mut btn: bool = true;
// ASL_DATA: name=rise type=BOOL scope=VAR unit=-
let mut rise: bool = false;
// ASL_DATA: name=fall type=BOOL scope=VAR unit=-
let mut fall: bool = false;
// ASL_DATA: name=q type=BOOL scope=VAR unit=-
let mut q: bool = false;

fn setup() {
  pin_mode(btnPin, INPUT);
  pin_mode(ledPin, OUTPUT);
}

fn loop_() {
  rise = R_TRIG("rt1", btn);
  fall = F_TRIG("ft1", btn);
  q = TOF("tof1", btn, PT);
  digital_write(ledPin, q);
  btn = false;
  delay_ms(STEP);
}
