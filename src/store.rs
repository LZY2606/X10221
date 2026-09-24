

#[derive(Debug, Clone)]
pub struct CandidateRec {
    pub id: String,
    pub population: String,
    pub label: String,
    pub status: String,
    pub value: Value,
    pub provenance: Vec<ProvenanceRef>,
    pub denominator: Option<Value>,
    pub event_seq: i64,
}

#[derive(Debug, Clone)]
pub struct DiagnosticRow {
    pub id: String,
    pub object_id: String,
    pub severity: String,
    pub code: String,
    pub message: String,
    pub event_seq: i64,
}
