pub const PROJECT_NAME: &str = "Database Backup Utility";

#[cfg(test)]
mod tests {
    use super::PROJECT_NAME;

    #[test]
    fn project_name_is_not_empty() {
        assert!(!PROJECT_NAME.is_empty());
    }
}
