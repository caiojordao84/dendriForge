# DendriForge MicroPython frontend — MVP subset
# project: S02-ton-on-delay
from machine import Pin
from time import sleep_ms

# ASL_HW: pin=8 type=Digital mode=Di label=startBtn role=digital-in
START_BTN = 8
# ASL_HW: pin=13 type=Digital mode=Do label=outLed role=digital-out
OUT_LED = 13

# ASL_DATA: name=PT type=TIME scope=CONST unit=ms
PT = 200
# ASL_DATA: name=STEP type=TIME scope=CONST unit=ms
STEP = 100
# ASL_DATA: name=start type=BOOL scope=VAR unit=-
start = True
# ASL_DATA: name=q type=BOOL scope=VAR unit=-
q = False

start_btn_pin = None
out_led_pin = None

def setup():
    global start_btn_pin
    global out_led_pin
    start_btn_pin = Pin(START_BTN, Pin.IN)
    out_led_pin = Pin(OUT_LED, Pin.OUT)
    out_led_pin.value(0)

def loop():
    global q
    q = TON("t1", start, PT)
    out_led_pin.value(q)
    sleep_ms(STEP)

setup()
while True:
    loop()
