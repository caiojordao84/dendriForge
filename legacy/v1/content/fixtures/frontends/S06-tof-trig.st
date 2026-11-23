(* DendriForge ST frontend — MVP subset *)
(* project: S06-iec-tof-trig *)
PROGRAM MAIN
(* hardware *)
(* HW: pin=2 type=Digital mode=Di label=btnPin role=digital-in *)
(* HW: pin=13 type=Digital mode=Do label=ledPin role=digital-out *)
VAR
    CONSTANT PT : TIME := T#200ms;
    CONSTANT STEP : TIME := T#100ms;
    btn : BOOL := TRUE;
    rise : BOOL := FALSE;
    fall : BOOL := FALSE;
    q : BOOL := FALSE;
END_VAR

SETUP
    CFG_DIGITAL_INPUT(2);
    CFG_DIGITAL_OUTPUT(13);

LOOP
    rise := R_TRIG('rt1', btn);
    fall := F_TRIG('ft1', btn);
    q := TOF('tof1', btn, PT);
    SET_BOOL(13, q);
    btn := FALSE;
    WAIT_TIME(STEP);
END_PROGRAM
