(* DendriForge ST frontend — MVP subset *)
(* project: S07-iec-counters *)
PROGRAM MAIN
VAR
    pulse : BOOL := FALSE;
    reset : BOOL := FALSE;
    load : BOOL := FALSE;
    CONSTANT pv : INT := 3;
    qu : BOOL := FALSE;
    qd : BOOL := FALSE;
    ctu1_CV : INT := 0;
    ctd1_CV : INT := 3;
    ctud1_CV : INT := 0;
    ctud1_QD : BOOL := FALSE;
END_VAR

SETUP

LOOP
    pulse := FALSE;
    qu := CTU('ctu1', pulse, reset, pv);
    qd := CTD('ctd1', pulse, load, pv);
    qu := CTUD('ctud1', pulse, FALSE, reset, load, pv);
    pulse := TRUE;
    qu := CTU('ctu1', pulse, reset, pv);
    qd := CTD('ctd1', pulse, load, pv);
    qu := CTUD('ctud1', pulse, FALSE, reset, load, pv);
END_PROGRAM
