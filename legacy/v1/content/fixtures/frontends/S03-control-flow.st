(* DendriForge ST frontend — MVP subset *)
(* project: S03-control-flow-get-bool *)
PROGRAM MAIN
(* hardware *)
(* HW: pin=2 type=Digital mode=Di label=btnPin role=digital-in *)
(* HW: pin=13 type=Digital mode=Do label=ledPin role=digital-out *)
VAR
    CONSTANT PULSE : TIME := T#50ms;
    CONSTANT LIMIT : UINT := 3;
    pressed : BOOL := FALSE;
    count : INT := 0;
    mode : UINT := 0;
END_VAR

SETUP
    CFG_DIGITAL_INPUT(2);
    CFG_DIGITAL_OUTPUT(13);
    SET_BOOL(13, FALSE);

LOOP
    pressed := GET_BOOL(2);
    IF pressed THEN
        mode := 1;
        SET_BOOL(13, TRUE);
    ELSIF (mode = 1) THEN
        mode := 2;
        SET_BOOL(13, FALSE);
    ELSE
        FOR i := 0 TO LIMIT - 1 DO
            count := i;
        END_FOR;
        SET_BOOL(13, FALSE);
    END_IF;
    WAIT_TIME(PULSE);
END_PROGRAM
