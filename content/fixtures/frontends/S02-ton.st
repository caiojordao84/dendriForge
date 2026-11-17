(* DendriForge ST frontend — MVP subset *)
(* project: S02-ton-on-delay *)
PROGRAM MAIN
(* hardware *)
(* HW: pin=8 type=Digital mode=Di label=startBtn role=digital-in *)
(* HW: pin=13 type=Digital mode=Do label=outLed role=digital-out *)
VAR
    CONSTANT PT : TIME := T#200ms;
    CONSTANT STEP : TIME := T#100ms;
    start : BOOL := TRUE;
    q : BOOL := FALSE;
END_VAR

SETUP
    CFG_DIGITAL_INPUT(8);
    CFG_DIGITAL_OUTPUT(13);
    SET_BOOL(13, FALSE);

LOOP
    q := TON('t1', start, PT);
    SET_BOOL(13, q);
    WAIT_TIME(STEP);
END_PROGRAM
