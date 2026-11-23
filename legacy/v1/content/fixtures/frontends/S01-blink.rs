// DendriForge Rust frontend — P2 Dendri HAL subset
// project: S01-blink-set-bool-wait

// ASL_HW: pin=13 type=Digital mode=Do label=ledPin role=digital-out
const ledPin: u32 = 13;

// ASL_DATA: name=ON_TIME type=TIME scope=CONST unit=ms
const ON_TIME: u32 = 100;
// ASL_DATA: name=OFF_TIME type=TIME scope=CONST unit=ms
const OFF_TIME: u32 = 100;

fn setup() {
  pin_mode(ledPin, OUTPUT);
  digital_write(ledPin, LOW);
}

fn loop_() {
  digital_write(ledPin, HIGH);
  delay_ms(ON_TIME);
  digital_write(ledPin, LOW);
  delay_ms(OFF_TIME);
}
