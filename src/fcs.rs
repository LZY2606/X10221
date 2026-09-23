//! Minimal FCS 3.1 reader/writer for the offline flow boundary case.
//!
//! Only the TEXT/HEADER/DATA segments needed by the bundled case are
//! supported. Real platform export still arrives through the JSON case
//! package; this module proves raw FCS bytes round-trip in tests.
