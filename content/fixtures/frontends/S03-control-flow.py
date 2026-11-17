# DendriForge MicroPython frontend — MVP subset
# project: S03-control-flow-get-bool
from machine import Pin
from time import sleep_ms

# ASL_HW: pin=2 type=Digital mode=Di label=btnPin role=digital-in
BTN_PIN = 2
# ASL_HW: pin=13 type=Digital mode=Do label=ledPin role=digital-out
LED_PIN = 13

# ASL_DATA: name=PULSE type=TIME scope=CONST unit=ms
PULSE = 50
# ASL_DATA: name=LIMIT type=UINT scope=CONST unit=-
LIMIT = 3
# ASL_DATA: name=pressed type=BOOL scope=VAR unit=-
pressed = False
# ASL_DATA: name=count type=INT scope=VAR unit=-
count = 0
# ASL_DATA: name=mode type=UINT scope=VAR unit=-
mode = 0

btn_pin = None
led_pin = None

def setup():
    global btn_pin
    global led_pin
    btn_pin = Pin(BTN_PIN, Pin.IN)
    led_pin = Pin(LED_PIN, Pin.OUT)
    led_pin.value(0)

def loop():
    global pressed
    global mode
    global count
    pressed = btn_pin.value()
    if pressed:
        mode = 1
        led_pin.value(1)
    elif (mode == 1):
        mode = 2
        led_pin.value(0)
    else:
        for i in range(LIMIT):
            count = i
        led_pin.value(0)
    sleep_ms(PULSE)

setup()
while True:
    loop()
