use super::*;

fn model_with(
    rows: usize,
    fields: usize,
    actions: usize,
) -> Result<ScriptClientViewModel, ScriptDtoError> {
    let rows = (0..rows)
        .map(|_| ScriptClientViewRow::try_new(vec!["cell".to_owned()]).unwrap())
        .collect();
    let fields = (0..fields)
        .map(|index| ScriptClientViewField::try_number(&format!("field-{index}"), 1.0).unwrap())
        .collect();
    let actions = (0..actions)
        .map(|index| {
            ScriptClientViewAction::try_new(&format!("action-{index}"), true, None, None).unwrap()
        })
        .collect();
    ScriptClientViewModel::try_new(
        0,
        1,
        rows,
        fields,
        actions,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None,
    )
}

#[test]
fn model_enforces_every_frozen_bound() {
    assert!(model_with(MAX_CLIENT_VIEW_ROWS, 0, 0).is_ok());
    assert!(matches!(
        model_with(MAX_CLIENT_VIEW_ROWS + 1, 0, 0),
        Err(ScriptDtoError::TooManyEntries {
            field: "view rows",
            ..
        })
    ));
    assert!(model_with(0, MAX_CLIENT_VIEW_FIELDS, 0).is_ok());
    assert!(matches!(
        model_with(0, MAX_CLIENT_VIEW_FIELDS + 1, 0),
        Err(ScriptDtoError::TooManyEntries {
            field: "view fields",
            ..
        })
    ));
    assert!(model_with(0, 0, MAX_CLIENT_VIEW_ACTIONS).is_ok());
    assert!(matches!(
        model_with(0, 0, MAX_CLIENT_VIEW_ACTIONS + 1),
        Err(ScriptDtoError::TooManyEntries {
            field: "view actions",
            ..
        })
    ));
}

#[test]
fn model_rejects_out_of_range_page_and_oversized_strings() {
    assert!(matches!(
        ScriptClientViewModel::try_new(
            1,
            1,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None
        ),
        Err(ScriptDtoError::InvalidBounds)
    ));
    let long = "x".repeat(MAX_CLIENT_VIEW_CELL_BYTES + 1);
    assert!(matches!(
        ScriptClientViewRow::try_new(vec![long]),
        Err(ScriptDtoError::ValueTooLong { .. })
    ));
    let title = "y".repeat(MAX_CLIENT_VIEW_TITLE_BYTES + 1);
    assert!(matches!(
        ScriptClientViewAction::try_new("action", true, Some(title), None),
        Err(ScriptDtoError::ValueTooLong { .. })
    ));
    assert!(matches!(
        ScriptClientViewField::try_number("field", f64::INFINITY),
        Err(ScriptDtoError::InvalidAmount)
    ));
}

#[test]
fn model_rejects_duplicate_ids_and_tokens() {
    let field = ScriptClientViewField::try_number("count", 1.0).unwrap();
    assert!(matches!(
        ScriptClientViewModel::try_new(
            0,
            1,
            Vec::new(),
            vec![field.clone(), field],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None
        ),
        Err(ScriptDtoError::DuplicateId {
            field: "view field",
            ..
        })
    ));
    let marker = |token: &str| {
        ScriptClientViewMarker::try_new(
            "anchor",
            Some(token.to_owned()),
            Some("place".to_owned()),
            None,
            Some(4.0),
        )
        .unwrap()
    };
    assert!(matches!(
        ScriptClientViewModel::try_new(
            0,
            1,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![marker("ctx-1"), {
                let mut second = marker("ctx-1");
                second.marker_id = "anchor-2".to_owned();
                second
            }],
            None
        ),
        Err(ScriptDtoError::DuplicateId {
            field: "view selection token",
            ..
        })
    ));
}

#[test]
fn marker_requires_an_action_when_it_carries_a_selection_token() {
    assert!(matches!(
        ScriptClientViewMarker::try_new("anchor", Some("ctx-1".to_owned()), None, None, None),
        Err(ScriptDtoError::EmptyValue { .. })
    ));
}

#[test]
fn selection_constraints_enforce_range_and_ttl() {
    assert!(
        ScriptClientSelectionConstraints::try_new(
            "minecraft:overworld",
            MAX_CLIENT_VIEW_RANGE_LIMIT,
            MAX_CLIENT_VIEW_SELECTION_TTL_TICKS,
            Some(ScriptClientViewFormation::Line),
            Some(4.0),
        )
        .is_ok()
    );
    assert!(
        ScriptClientSelectionConstraints::try_new("minecraft:overworld", 0, 1, None, None).is_err()
    );
    assert!(
        ScriptClientSelectionConstraints::try_new(
            "minecraft:overworld",
            MAX_CLIENT_VIEW_RANGE_LIMIT + 1,
            1,
            None,
            None,
        )
        .is_err()
    );
    assert!(
        ScriptClientSelectionConstraints::try_new(
            "minecraft:overworld",
            1,
            MAX_CLIENT_VIEW_SELECTION_TTL_TICKS + 1,
            None,
            None,
        )
        .is_err()
    );
}
