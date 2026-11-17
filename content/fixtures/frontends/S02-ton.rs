// DendriForge Rust frontend — P2 Dendri HAL subset
// project: S02-ton-on-delay

// ASL_HW: pin=8 type=Digital mode=Di label=startBtn role=digital-in
const startBtn: u32 = 8;
// ASL_HW: pin=13 type=Digital mode=Do label=outLed role=digital-out
const outLed: u32 = 13;

// ASL_DATA: name=PT type=TIME scope=CONST unit=ms
const PT: u32 = 200;
// ASL_DATA: name=STEP type=TIME scope=CONST unit=ms
const STEP: u32 = 100;
// ASL_DATA: name=start type=BOOL scope=VAR unit=-
let mut start: bool = true;
// ASL_DATA: name=q type=BOOL scope=VAR unit=-
let mut q: bool = false;

fn setup() {
  pin_mode(startBtn, INPUT);
  pin_mode(outLed, OUTPUT);
  digital_write(outLed, LOW);
}

fn loop_() {
  q = TON("t1", start, PT);
  digital_write(outLed, q);
  delay_ms(STEP);
}
