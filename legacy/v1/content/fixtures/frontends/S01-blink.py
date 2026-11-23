# DendriForge MicroPython frontend — MVP subset
# project: S01-blink-set-bool-wait
from machine import Pin
from time import sleep_ms

# ASL_HW: pin=13 type=Digital mode=Do label=ledPin role=digital-out
LED_PIN = 13

# ASL_DATA: name=ON_TIME type=TIME scope=CONST unit=ms
ON_TIME = 100
# ASL_DATA: name=OFF_TIME type=TIME scope=CONST unit=ms
OFF_TIME = 100

led_pin = None

def setup():
    global led_pin
    led_pin = Pin(LED_PIN, Pin.OUT)
    led_pin.value(0)

def loop():
    led_pin.value(1)
    sleep_ms(ON_TIME)
    led_pin.value(0)
    sleep_ms(OFF_TIME)

setup()
while True:
    loop()
