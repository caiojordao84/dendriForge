(* DendriForge ST frontend — MVP subset *)
(* project: S05-uart-i2c-spi *)
PROGRAM MAIN
(* hardware *)
(* HW: pin=21 type=Digital mode=Bi label=sdaPin role=i2c-sda *)
(* HW: pin=22 type=Digital mode=Bi label=sclPin role=i2c-scl *)
(* HW: pin=23 type=Digital mode=Do label=mosiPin role=spi-mosi *)
(* HW: pin=19 type=Digital mode=Di label=misoPin role=spi-miso *)
(* HW: pin=18 type=Digital mode=Do label=sckPin role=spi-sck *)
(* HW: pin=5 type=Digital mode=Do label=csPin role=spi-cs *)
VAR
    CONSTANT msg : STRING := 'telemetry';
END_VAR

SETUP
    CFG_SERIAL(115200);
    CFG_I2C(0, 21, 22, 400000);
    CFG_SPI(1, 23, 19, 18, 5);

LOOP
    SERIAL_PRINTLN(msg);
END_PROGRAM
