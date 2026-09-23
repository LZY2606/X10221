//! Parser for the FCS `$SPILL` style spillover CSV.
//!
//! Shape: `n,<detectors...>,<fluorochromes...>` followed by n*n rows. This is
//! the same encoding flow platforms put in the FCS keyword, so the offline
//! case pack uses it rather than a generic table.

use crate::domain::Diagnostic;

#[derive(Debug, Clone)]
pub struct SpillTable {
    pub detectors: Vec<String>,
    pub fluorochromes: Vec<String>,
    pub matrix: Vec<Vec<f64>>,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn parse_spill_csv(text: &str) -> Result<SpillTable, String> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return Err("empty spillover table".into());
    }
    let header: Vec<&str> = lines[0].split(',').map(str::trim).collect();
    if header.len() < 2 {
        return Err("spill header missing".into());
    }
    let n: usize = header[0]
        .parse()
        .map_err(|_| format!("bad spill dimension {:?}", header[0]))?;
    if header.len() != 1 + 2 * n {
        return Err(format!(
            "spill header has {} fields, expected {}",
            header.len(),
            1 + 2 * n
        ));
    }
    let detectors = header[1..=n].iter().map(|s| s.to_string()).collect();
    let fluorochromes = header[1 + n..].iter().map(|s| s.to_string()).collect();
    let mut diagnostics = Vec::new();
    let mut matrix = Vec::new();
    for (i, line) in lines.iter().enumerate().skip(1).take(n) {
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        if fields.len() != n {
            return Err(format!("spill row {} has {} fields, expected {}", i, fields.len(), n));
        }
        let mut row = Vec::new();
        for f in fields {
            match f.parse::<f64>() {
                Ok(v) => row.push(v),
                Err(_) => {
                    diagnostics.push(Diagnostic {
                        code: "spill.cell.unparsable".into(),
                        message: format!("spill cell {:?} left in place as 0 with diagnostic", f),
                        raw_repr: f.to_string(),
                    });
                    row.push(0.0);
                }
            }
        }
        matrix.push(row);
    }
    if matrix.len() != n {
        return Err(format!("spill has {} data rows, expected {}", matrix.len(), n));
    }
    Ok(SpillTable {
        detectors,
        fluorochromes,
        matrix,
        diagnostics,
    })
}
