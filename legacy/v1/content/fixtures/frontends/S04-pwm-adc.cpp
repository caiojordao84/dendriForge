// DendriForge C++ frontend — MVP Arduino subset
// project: S04-pwm-adc-map

// ASL_HW: pin=34 type=Analog mode=Di label=potPin role=analog-in
const int potPin = 34;
// ASL_HW: pin=25 type=Digital mode=Do label=fanPwm role=pwm-out
const int fanPwm = 25;

// ASL_DATA: name=raw type=UINT scope=VAR unit=-
unsigned int raw = 0;
// ASL_DATA: name=duty type=UINT scope=VAR unit=-
unsigned int duty = 0;

void setup() {
  CFG_ADC_INPUT(potPin);
  CFG_PWM_OUTPUT(fanPwm, 5000);
  analogWrite(fanPwm, 0);
}

void loop() {
  raw = analogRead(potPin);
  duty = REAL_TO_INT(MAP_RANGE(raw, 0, 65535, 0, 255));
  analogWrite(fanPwm, duty);
}
