pub fn registration_trial_plan_code(trial_days: u32) -> Option<&'static str> {
    match trial_days {
        7 => Some("professional_free_trial_7_days"),
        21 => Some("collaborator_free_trial_7_days"),
        _ => None,
    }
}
