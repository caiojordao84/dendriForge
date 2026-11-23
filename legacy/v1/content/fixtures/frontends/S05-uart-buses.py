# DendriForge MicroPython frontend — MVP subset
# project: S05-uart-i2c-spi
from machine import Pin, I2C, SPI, UART
from time import sleep_ms

# ASL_HW: pin=21 type=Digital mode=Bi label=sdaPin role=i2c-sda
SDA_PIN = 21
# ASL_HW: pin=22 type=Digital mode=Bi label=sclPin role=i2c-scl
SCL_PIN = 22
# ASL_HW: pin=23 type=Digital mode=Do label=mosiPin role=spi-mosi
MOSI_PIN = 23
# ASL_HW: pin=19 type=Digital mode=Di label=misoPin role=spi-miso
MISO_PIN = 19
# ASL_HW: pin=18 type=Digital mode=Do label=sckPin role=spi-sck
SCK_PIN = 18
# ASL_HW: pin=5 type=Digital mode=Do label=csPin role=spi-cs
CS_PIN = 5

# ASL_DATA: name=msg type=STRING scope=CONST unit=-
msg = "telemetry"

sda_pin = None
scl_pin = None
mosi_pin = None
miso_pin = None
sck_pin = None
cs_pin = None

def setup():
    global sda_pin
    global scl_pin
    global mosi_pin
    global miso_pin
    global sck_pin
    global cs_pin
    uart_0 = UART(0, baudrate=115200)
    CFG_I2C(0, 21, 22, 400000)
    CFG_SPI(1, 23, 19, 18, 5)

def loop():
    uart_0.write(msg + "\n")

setup()
while True:
    loop()
