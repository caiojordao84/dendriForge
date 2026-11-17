// DendriForge Rust frontend — P2 Dendri HAL subset
// project: S07-iec-counters

// ASL_DATA: name=pulse type=BOOL scope=VAR unit=-
let mut pulse: bool = false;
// ASL_DATA: name=reset type=BOOL scope=VAR unit=-
let mut reset: bool = false;
// ASL_DATA: name=load type=BOOL scope=VAR unit=-
let mut load: bool = false;
// ASL_DATA: name=pv type=INT scope=CONST unit=-
const pv: i32 = 3;
// ASL_DATA: name=qu type=BOOL scope=VAR unit=-
let mut qu: bool = false;
// ASL_DATA: name=qd type=BOOL scope=VAR unit=-
let mut qd: bool = false;
// ASL_DATA: name=ctu1_CV type=INT scope=VAR unit=-
let mut ctu1_CV: i32 = 0;
// ASL_DATA: name=ctd1_CV type=INT scope=VAR unit=-
let mut ctd1_CV: i32 = 3;
// ASL_DATA: name=ctud1_CV type=INT scope=VAR unit=-
let mut ctud1_CV: i32 = 0;
// ASL_DATA: name=ctud1_QD type=BOOL scope=VAR unit=-
let mut ctud1_QD: bool = false;

fn setup() {
}

fn loop_() {
  pulse = false;
  qu = CTU("ctu1", pulse, reset, pv);
  qd = CTD("ctd1", pulse, load, pv);
  qu = CTUD("ctud1", pulse, false, reset, load, pv);
  pulse = true;
  qu = CTU("ctu1", pulse, reset, pv);
  qd = CTD("ctd1", pulse, load, pv);
  qu = CTUD("ctud1", pulse, false, reset, load, pv);
}
