// DendriForge C++ frontend — MVP Arduino subset
// project: S07-iec-counters

// ASL_DATA: name=pulse type=BOOL scope=VAR unit=-
bool pulse = false;
// ASL_DATA: name=reset type=BOOL scope=VAR unit=-
bool reset = false;
// ASL_DATA: name=load type=BOOL scope=VAR unit=-
bool load = false;
// ASL_DATA: name=pv type=INT scope=CONST unit=-
const int pv = 3;
// ASL_DATA: name=qu type=BOOL scope=VAR unit=-
bool qu = false;
// ASL_DATA: name=qd type=BOOL scope=VAR unit=-
bool qd = false;
// ASL_DATA: name=ctu1_CV type=INT scope=VAR unit=-
int ctu1_CV = 0;
// ASL_DATA: name=ctd1_CV type=INT scope=VAR unit=-
int ctd1_CV = 3;
// ASL_DATA: name=ctud1_CV type=INT scope=VAR unit=-
int ctud1_CV = 0;
// ASL_DATA: name=ctud1_QD type=BOOL scope=VAR unit=-
bool ctud1_QD = false;

void setup() {
}

void loop() {
  pulse = false;
  qu = CTU("ctu1", pulse, reset, pv);
  qd = CTD("ctd1", pulse, load, pv);
  qu = CTUD("ctud1", pulse, false, reset, load, pv);
  pulse = true;
  qu = CTU("ctu1", pulse, reset, pv);
  qd = CTD("ctd1", pulse, load, pv);
  qu = CTUD("ctud1", pulse, false, reset, load, pv);
}
