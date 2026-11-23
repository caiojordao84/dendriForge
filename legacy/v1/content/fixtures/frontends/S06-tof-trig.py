# DendriForge MicroPython frontend — MVP subset
# project: S06-iec-tof-trig
from machine import Pin
from time import sleep_ms

# ASL_HW: pin=2 type=Digital mode=Di label=btnPin role=digital-in
BTN_PIN = 2
# ASL_HW: pin=13 type=Digital mode=Do label=ledPin role=digital-out
LED_PIN = 13

# ASL_DATA: name=PT type=TIME scope=CONST unit=ms
PT = 200
# ASL_DATA: name=STEP type=TIME scope=CONST unit=ms
STEP = 100
# ASL_DATA: name=btn type=BOOL scope=VAR unit=-
btn = True
# ASL_DATA: name=rise type=BOOL scope=VAR unit=-
rise = False
# ASL_DATA: name=fall type=BOOL scope=VAR unit=-
fall = False
# ASL_DATA: name=q type=BOOL scope=VAR unit=-
q = False

btn_pin = None
led_pin = None

def setup():
    global btn_pin
    global led_pin
    btn_pin = Pin(BTN_PIN, Pin.IN)
    led_pin = Pin(LED_PIN, Pin.OUT)

def loop():
    global rise
    global fall
    global q
    global btn
    rise = R_TRIG("rt1", btn)
    fall = F_TRIG("ft1", btn)
    q = TOF("tof1", btn, PT)
    led_pin.value(q)
    btn = False
    sleep_ms(STEP)

setup()
while True:
    loop()
