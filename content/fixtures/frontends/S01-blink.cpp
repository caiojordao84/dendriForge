// DendriForge C++ frontend — MVP Arduino subset
// project: S01-blink-set-bool-wait

// ASL_HW: pin=13 type=Digital mode=Do label=ledPin role=digital-out
const int ledPin = 13;

// ASL_DATA: name=ON_TIME type=TIME scope=CONST unit=ms
const unsigned long ON_TIME = 100UL;
// ASL_DATA: name=OFF_TIME type=TIME scope=CONST unit=ms
const unsigned long OFF_TIME = 100UL;

void setup() {
  pinMode(ledPin, OUTPUT);
  digitalWrite(ledPin, LOW);
}

void loop() {
  digitalWrite(ledPin, HIGH);
  delay(ON_TIME);
  digitalWrite(ledPin, LOW);
  delay(OFF_TIME);
}
