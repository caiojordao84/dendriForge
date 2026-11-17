// DendriForge Rust frontend — P2 Dendri HAL subset
// project: S03-control-flow-get-bool

// ASL_HW: pin=2 type=Digital mode=Di label=btnPin role=digital-in
const btnPin: u32 = 2;
// ASL_HW: pin=13 type=Digital mode=Do label=ledPin role=digital-out
const ledPin: u32 = 13;

// ASL_DATA: name=PULSE type=TIME scope=CONST unit=ms
const PULSE: u32 = 50;
// ASL_DATA: name=LIMIT type=UINT scope=CONST unit=-
const LIMIT: u32 = 3;
// ASL_DATA: name=pressed type=BOOL scope=VAR unit=-
let mut pressed: bool = false;
// ASL_DATA: name=count type=INT scope=VAR unit=-
let mut count: i32 = 0;
// ASL_DATA: name=mode type=UINT scope=VAR unit=-
let mut mode: u32 = 0;

fn setup() {
  pin_mode(btnPin, INPUT);
  pin_mode(ledPin, OUTPUT);
  digital_write(ledPin, LOW);
}

fn loop_() {
  pressed = digital_read(btnPin);
  if pressed {
    mode = 1;
    digital_write(ledPin, HIGH);
  } else if (mode == 1) {
    mode = 2;
    digital_write(ledPin, LOW);
  } else {
    for i in 0..LIMIT {
      count = i;
    }
    digital_write(ledPin, LOW);
  }
  delay_ms(PULSE);
}
