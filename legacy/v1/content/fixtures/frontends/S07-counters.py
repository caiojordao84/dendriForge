# DendriForge MicroPython frontend — MVP subset
# project: S07-iec-counters
from machine import Pin
from time import sleep_ms

# ASL_DATA: name=pulse type=BOOL scope=VAR unit=-
pulse = False
# ASL_DATA: name=reset type=BOOL scope=VAR unit=-
reset = False
# ASL_DATA: name=load type=BOOL scope=VAR unit=-
load = False
# ASL_DATA: name=pv type=INT scope=CONST unit=-
pv = 3
# ASL_DATA: name=qu type=BOOL scope=VAR unit=-
qu = False
# ASL_DATA: name=qd type=BOOL scope=VAR unit=-
qd = False
# ASL_DATA: name=ctu1_CV type=INT scope=VAR unit=-
ctu1_CV = 0
# ASL_DATA: name=ctd1_CV type=INT scope=VAR unit=-
ctd1_CV = 3
# ASL_DATA: name=ctud1_CV type=INT scope=VAR unit=-
ctud1_CV = 0
# ASL_DATA: name=ctud1_QD type=BOOL scope=VAR unit=-
ctud1_QD = False

def setup():
    pass

def loop():
    global pulse
    global qu
    global qd
    pulse = False
    qu = CTU("ctu1", pulse, reset, pv)
    qd = CTD("ctd1", pulse, load, pv)
    qu = CTUD("ctud1", pulse, False, reset, load, pv)
    pulse = True
    qu = CTU("ctu1", pulse, reset, pv)
    qd = CTD("ctd1", pulse, load, pv)
    qu = CTUD("ctud1", pulse, False, reset, load, pv)

setup()
while True:
    loop()
