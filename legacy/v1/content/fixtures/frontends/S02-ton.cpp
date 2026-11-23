// DendriForge C++ frontend — MVP Arduino subset
// project: S02-ton-on-delay

// ASL_HW: pin=8 type=Digital mode=Di label=startBtn role=digital-in
const int startBtn = 8;
// ASL_HW: pin=13 type=Digital mode=Do label=outLed role=digital-out
const int outLed = 13;

// ASL_DATA: name=PT type=TIME scope=CONST unit=ms
const unsigned long PT = 200UL;
// ASL_DATA: name=STEP type=TIME scope=CONST unit=ms
const unsigned long STEP = 100UL;
// ASL_DATA: name=start type=BOOL scope=VAR unit=-
bool start = true;
// ASL_DATA: name=q type=BOOL scope=VAR unit=-
bool q = false;

void setup() {
  pinMode(startBtn, INPUT);
  pinMode(outLed, OUTPUT);
  digitalWrite(outLed, LOW);
}

void loop() {
  q = TON("t1", start, PT);
  digitalWrite(outLed, q);
  delay(STEP);
}
