// DendriForge C++ frontend — MVP Arduino subset
// project: S05-uart-i2c-spi

// ASL_HW: pin=21 type=Digital mode=Bi label=sdaPin role=i2c-sda
const int sdaPin = 21;
// ASL_HW: pin=22 type=Digital mode=Bi label=sclPin role=i2c-scl
const int sclPin = 22;
// ASL_HW: pin=23 type=Digital mode=Do label=mosiPin role=spi-mosi
const int mosiPin = 23;
// ASL_HW: pin=19 type=Digital mode=Di label=misoPin role=spi-miso
const int misoPin = 19;
// ASL_HW: pin=18 type=Digital mode=Do label=sckPin role=spi-sck
const int sckPin = 18;
// ASL_HW: pin=5 type=Digital mode=Do label=csPin role=spi-cs
const int csPin = 5;

// ASL_DATA: name=msg type=STRING scope=CONST unit=-
const String msg = "telemetry";

void setup() {
  Serial.begin(115200);
  CFG_I2C(0, 21, 22, 400000);
  CFG_SPI(1, 23, 19, 18, 5);
}

void loop() {
  Serial.println(msg);
}
