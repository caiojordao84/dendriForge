# DendriForge MicroPython frontend — MVP subset
# project: S04-pwm-adc-map
from machine import Pin, PWM, ADC
from time import sleep_ms

# ASL_HW: pin=34 type=Analog mode=Di label=potPin role=analog-in
POT_PIN = 34
# ASL_HW: pin=25 type=Digital mode=Do label=fanPwm role=pwm-out
FAN_PWM = 25

# ASL_DATA: name=raw type=UINT scope=VAR unit=-
raw = 0
# ASL_DATA: name=duty type=UINT scope=VAR unit=-
duty = 0

pot_pin = None
fan_pwm_pin = None

def setup():
    global pot_pin
    global fan_pwm_pin
    pot_adc = ADC(Pin(POT_PIN))
    fan_pwm_pwm = PWM(Pin(FAN_PWM), freq=5000)
    fan_pwm_pwm.duty_u16(0)

def loop():
    global raw
    global duty
    raw = pot_adc.read_u16()
    duty = REAL_TO_INT(MAP_RANGE(raw, 0, 65535, 0, 255))
    fan_pwm_pwm.duty_u16(duty)

setup()
while True:
    loop()
