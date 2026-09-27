# Flow boundary-room offline case

This is a domain-specific fixed case for compensation, transform, gating lineage and population uncertainty. It is not a generic spreadsheet.

Initial import requires all six independently versioned sources:

- `initial/fcs_events.json`: detector ADC values, a saturation-range event, and an unconverted `OVRFLW` sentinel kept in place.
- `initial/spillover_metadata.json`: single-stain controls and the matrix reference.
- `initial/compensation_matrix.json`: a near-singular spillover matrix.
- `initial/transform_parameters.json`: channel-specific `asinh` cofactors.
- `initial/gate_polygons.json`: coordinates, channel basis and units.
- `initial/gate_lineage.json`: stable parent-child edges.

Replay revisions:

1. Commit `compensation_matrix_v2.json` together with `transform_parameters_v2.json` to produce an invertible parameter revision.
2. Adjudicate the two candidates created by boundary event `E03`.
3. Commit `gate_polygons_v2.json`; the G02 and G03 conclusions are invalidated but never overwritten.
4. Accept the new G01 candidate to recompute descendants locally.
5. Commit `gate_lineage_v2_rename.json`; stable `L02/G02` identity remains fixed while only the display label changes.
6. Fork a second branch from the same pre-decision head and accept the other candidate, then submit the two branches as conflicting human decisions.

The package is embedded by the application and available through the local review UI.
