# DendriForge MicroPython frontend — MVP subset
# project: S01-blink-set-bool-wait
from machine import Pin
from time import sleep_ms

LED_PIN = 13

ON_TIME = 100
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
