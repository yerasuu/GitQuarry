use super::*;

fn node<'a>(id: &'a str, image: &'a str, needs: &'a [String]) -> JobNode<'a> {
    JobNode { id, image, needs }
}

#[test]
fn accepts_a_valid_linear_graph() {
    let needs_a: Vec<String> = vec![];
    let needs_b = vec!["a".to_string()];
    let nodes = vec![node("a", "rust:1.80", &needs_a), node("b", "rust:1.80", &needs_b)];
    assert!(validate_graph(&nodes, DEFAULT_MAX_JOBS).is_ok());
}

#[test]
fn rejects_duplicate_ids() {
    let empty: Vec<String> = vec![];
    let nodes = vec![node("a", "img", &empty), node("a", "img", &empty)];
    let errors = validate_graph(&nodes, DEFAULT_MAX_JOBS).unwrap_err();
    assert!(errors.contains(&GraphValidationError::DuplicateId("a".to_string())));
}

#[test]
fn rejects_dangling_needs() {
    let needs = vec!["ghost".to_string()];
    let nodes = vec![node("a", "img", &needs)];
    let errors = validate_graph(&nodes, DEFAULT_MAX_JOBS).unwrap_err();
    assert!(errors.contains(&GraphValidationError::UnknownNeed {
        job: "a".to_string(),
        needs: "ghost".to_string(),
    }));
}

#[test]
fn rejects_cycles() {
    let needs_a = vec!["b".to_string()];
    let needs_b = vec!["a".to_string()];
    let nodes = vec![node("a", "img", &needs_a), node("b", "img", &needs_b)];
    let errors = validate_graph(&nodes, DEFAULT_MAX_JOBS).unwrap_err();
    assert!(errors.iter().any(|e| matches!(e, GraphValidationError::Cycle(_))));
}

#[test]
fn rejects_empty_image() {
    let empty: Vec<String> = vec![];
    let nodes = vec![node("a", "", &empty)];
    let errors = validate_graph(&nodes, DEFAULT_MAX_JOBS).unwrap_err();
    assert!(errors.contains(&GraphValidationError::EmptyImage("a".to_string())));
}

#[test]
fn rejects_too_many_jobs() {
    let empty: Vec<String> = vec![];
    let nodes = vec![node("a", "img", &empty), node("b", "img", &empty)];
    let errors = validate_graph(&nodes, 1).unwrap_err();
    assert!(errors.contains(&GraphValidationError::TooManyJobs { count: 2, max: 1 }));
}

#[test]
fn a_dangling_need_does_not_also_report_as_a_cycle() {
    let needs = vec!["ghost".to_string()];
    let nodes = vec![node("a", "img", &needs)];
    let errors = validate_graph(&nodes, DEFAULT_MAX_JOBS).unwrap_err();
    assert!(!errors.iter().any(|e| matches!(e, GraphValidationError::Cycle(_))));
}
