// DendriForge C++ frontend — MVP Arduino subset
// project: S06-iec-tof-trig

// ASL_HW: pin=2 type=Digital mode=Di label=btnPin role=digital-in
const int btnPin = 2;
// ASL_HW: pin=13 type=Digital mode=Do label=ledPin role=digital-out
const int ledPin = 13;

// ASL_DATA: name=PT type=TIME scope=CONST unit=ms
const unsigned long PT = 200UL;
// ASL_DATA: name=STEP type=TIME scope=CONST unit=ms
const unsigned long STEP = 100UL;
// ASL_DATA: name=btn type=BOOL scope=VAR unit=-
bool btn = true;
// ASL_DATA: name=rise type=BOOL scope=VAR unit=-
bool rise = false;
// ASL_DATA: name=fall type=BOOL scope=VAR unit=-
bool fall = false;
// ASL_DATA: name=q type=BOOL scope=VAR unit=-
bool q = false;

void setup() {
  pinMode(btnPin, INPUT);
  pinMode(ledPin, OUTPUT);
}

void loop() {
  rise = R_TRIG("rt1", btn);
  fall = F_TRIG("ft1", btn);
  q = TOF("tof1", btn, PT);
  digitalWrite(ledPin, q);
  btn = false;
  delay(STEP);
}
