// DendriForge Rust frontend — P2 Dendri HAL subset
// project: S05-uart-i2c-spi

// ASL_HW: pin=21 type=Digital mode=Bi label=sdaPin role=i2c-sda
const sdaPin: u32 = 21;
// ASL_HW: pin=22 type=Digital mode=Bi label=sclPin role=i2c-scl
const sclPin: u32 = 22;
// ASL_HW: pin=23 type=Digital mode=Do label=mosiPin role=spi-mosi
const mosiPin: u32 = 23;
// ASL_HW: pin=19 type=Digital mode=Di label=misoPin role=spi-miso
const misoPin: u32 = 19;
// ASL_HW: pin=18 type=Digital mode=Do label=sckPin role=spi-sck
const sckPin: u32 = 18;
// ASL_HW: pin=5 type=Digital mode=Do label=csPin role=spi-cs
const csPin: u32 = 5;

// ASL_DATA: name=msg type=STRING scope=CONST unit=-
const msg: &str = "telemetry";

fn setup() {
  serial_begin(115200);
  CFG_I2C(0, 21, 22, 400000);
  CFG_SPI(1, 23, 19, 18, 5);
}

fn loop_() {
  serial_println(msg);
}
