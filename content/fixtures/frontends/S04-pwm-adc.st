(* DendriForge ST frontend — MVP subset *)
(* project: S04-pwm-adc-map *)
PROGRAM MAIN
(* hardware *)
(* HW: pin=34 type=Analog mode=Di label=potPin role=analog-in *)
(* HW: pin=25 type=Digital mode=Do label=fanPwm role=pwm-out *)
VAR
    raw : UINT := 0;
    duty : UINT := 0;
END_VAR

SETUP
    CFG_ADC_INPUT(34);
    CFG_PWM_OUTPUT(25, 5000);
    SET_PWM_DUTY(25, 0);

LOOP
    raw := READ_ADC(34);
    duty := REAL_TO_INT(MAP_RANGE(raw, 0, 65535, 0, 255));
    SET_PWM_DUTY(25, duty);
END_PROGRAM
