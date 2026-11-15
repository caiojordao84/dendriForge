// DendriForge C++ frontend — MVP Arduino subset
// project: S01-blink-set-bool-wait

const int ledPin = 13;

const unsigned long ON_TIME = 100UL;
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
