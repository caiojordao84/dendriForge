// DendriForge C++ frontend — MVP Arduino subset
// project: S03-control-flow-get-bool

// ASL_HW: pin=2 type=Digital mode=Di label=btnPin role=digital-in
const int btnPin = 2;
// ASL_HW: pin=13 type=Digital mode=Do label=ledPin role=digital-out
const int ledPin = 13;

// ASL_DATA: name=PULSE type=TIME scope=CONST unit=ms
const unsigned long PULSE = 50UL;
// ASL_DATA: name=LIMIT type=UINT scope=CONST unit=-
const unsigned int LIMIT = 3;
// ASL_DATA: name=pressed type=BOOL scope=VAR unit=-
bool pressed = false;
// ASL_DATA: name=count type=INT scope=VAR unit=-
int count = 0;
// ASL_DATA: name=mode type=UINT scope=VAR unit=-
unsigned int mode = 0;

void setup() {
  pinMode(btnPin, INPUT);
  pinMode(ledPin, OUTPUT);
  digitalWrite(ledPin, LOW);
}

void loop() {
  pressed = digitalRead(btnPin);
  if (pressed) {
    mode = 1;
    digitalWrite(ledPin, HIGH);
  } else if ((mode == 1)) {
    mode = 2;
    digitalWrite(ledPin, LOW);
  } else {
    for (int i = 0; i < LIMIT; i++) {
      count = i;
    }
    digitalWrite(ledPin, LOW);
  }
  delay(PULSE);
}
