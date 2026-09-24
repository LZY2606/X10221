#[derive(Debug, Clone, PartialEq)]
pub struct MatrixReport {
    pub determinant: f64,
    pub reciprocal_condition: f64,
    pub singular: bool,
    pub near_singular: bool,
    pub inverse: Option<Vec<Vec<f64>>>,
    pub reason: String,
}

pub fn matrix_from_payload(value: &serde_json::Value) -> Vec<Vec<f64>> {
    value["matrix"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|row| {
            row.as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .filter_map(|x| x.as_f64())
                .collect()
        })
        .collect()
}

pub fn invert_two_by_two(matrix: &[Vec<f64>], near_threshold: f64) -> MatrixReport {
    let reason_ok = "invertible-matrix".to_string();
    if matrix.len() != 2 || matrix.iter().any(|row| row.len() != 2) {
        return report_fail(0.0, 0.0, true, "matrix is not a 2x2 square");
    }
    let a = matrix[0][0];
    let b = matrix[0][1];
    let c = matrix[1][0];
    let d = matrix[1][1];
    let det = a * d - b * c;
    if !det.is_finite() || det == 0.0 {
        return report_fail(det, 0.0, true, "singular compensation matrix has no inverse");
    }
    let norm1 = (a.abs() + c.abs()).max(b.abs() + d.abs());
    let inverse_norm = ((d.abs() + c.abs()).max(a.abs() + b.abs())) / det.abs();
    let reciprocal_condition = 1.0 / (norm1 * inverse_norm);
    if reciprocal_condition < near_threshold {
        return report_fail(
            det,
            reciprocal_condition,
            false,
            "near-singular compensation matrix must not be silently inverted",
        );
    }
    MatrixReport {
        determinant: det,
        reciprocal_condition,
        singular: false,
        near_singular: true,
        inverse: Some(vec![
            vec![d / det, -b / det],
            vec![-c / det, a / det],
        ]),
        reason: reason_ok,
    }
}

fn report_fail(
    determinant: f64,
    reciprocal_condition: f64,
    singular: bool,
    reason: &str,
) -> MatrixReport {
    MatrixReport {
        determinant,
        reciprocal_condition,
        singular,
        near_singular: !singular,
        inverse: None,
        reason: reason.to_string(),
    }
}

pub fn apply_inverse(matrix: &[Vec<f64>], values: &[f64]) -> Option<Vec<f64>> {
    let inverse = invert_two_by_two(matrix, 0.00000001).inverse?;
    Some(
        inverse
            .iter()
            .map(|row| row[0] * values[0] + row[1] * values[1])
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_exact_and_near_singular_matrices() {
        let exact = vec![vec![1.0, 2.0], vec![2.0, 4.0]];
        assert!(invert_two_by_two(&exact, 1e-8).inverse.is_none());
        assert!(invert_two_by_two(&exact, 1e-8).singular);

        let near = vec![vec![1.0, 1.0], vec![1.0, 1.0000001]];
        let report = invert_two_by_two(&near, 1e-8);
        assert!(report.inverse.is_none());
        assert!(report.near_singular);
    }

    #[test]
    fn inverts_well_conditioned_matrix() {
        let matrix = vec![vec![1.0, 0.2], vec![0.1, 1.0]];
        let report = invert_two_by_two(&matrix, 1e-8);
        let inverse = report.inverse.unwrap();
        assert!((inverse[0][0] - 1.0204081632653061).abs() < 1e-9);
    }
}
