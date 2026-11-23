(* DendriForge ST frontend — MVP subset *)
(* project: S01-blink-set-bool-wait *)
PROGRAM MAIN
(* hardware *)
(* HW: pin=13 type=Digital mode=Do label=ledPin role=digital-out *)
VAR
    CONSTANT ON_TIME : TIME := T#100ms;
    CONSTANT OFF_TIME : TIME := T#100ms;
END_VAR

SETUP
    CFG_DIGITAL_OUTPUT(13);
    SET_BOOL(13, FALSE);

LOOP
    SET_BOOL(13, TRUE);
    WAIT_TIME(ON_TIME);
    SET_BOOL(13, FALSE);
    WAIT_TIME(OFF_TIME);
END_PROGRAM
