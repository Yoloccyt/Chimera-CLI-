// Dependency Rules Inner Ring Test - Phase 1 Validation
// Purpose: Validate ADR-XXX fix for inner-ring boundary check logic bug
// Date: 2026-09-18

#[cfg(test)]
mod tests {
    use std::process::Command;

    /// TC-1: gsoe-evolution → decay-engine should NOT report GAP-A after script fix
    #[test]
    fn test_gsoe_to_decay_engine_not_flagged() {
        // Run the dependency rules check script
        let output = Command::new("powershell")
            .args(&[
                "-NoProfile",
                "-File",
                "scripts/check_dependency_rules.ps1"
            ])
            .current_dir("d:\\Chimera CLI")
            .output()
            .expect("Failed to execute script");

        let stdout = String::from_utf8_lossy(&output.stdout);
        
        // Verify that GAP-A is not reported for gsoe-evolution -> decay-engine
        assert!(!stdout.contains("[GAP-A] gsoe-evolution -> decay-engine"),
            "gsoe-evolution → decay-engine should NOT be flagged as GAP-A after script fix. Output:\n{}", 
            stdout);
        
        // Verify script exits successfully
        assert!(output.status.success(), 
            "check_dependency_rules.ps1 should exit with code 0. Output:\n{}", stdout);
    }

    /// TC-2: Full workspace dependency audit should EXIT=0
    #[test]
    fn test_full_workspace_audit_passes() {
        let output = Command::new("powershell")
            .args(&[
                "-NoProfile",
                "-File",
                "scripts/check_dependency_rules.ps1"
            ])
            .current_dir("d:\\Chimera CLI")
            .output()
            .expect("Failed to execute script");

        let stdout = String::from_utf8_lossy(&output.stdout);
        
        // Verify all checks pass
        assert!(stdout.contains("[OK] dependency iron-law audit all pass"),
            "All dependency checks should pass. Output:\n{}", stdout);
        
        assert!(output.status.success(), 
            "Script should exit with code 0 when all checks pass");
    }

    /// TC-3: cargo check --workspace should compile without errors
    #[test]
    fn test_cargo_check_workspace_passes() {
        let output = Command::new("cargo")
            .args(&["check", "--workspace"])
            .current_dir("d:\\Chimera CLI")
            .output()
            .expect("Failed to execute cargo check");

        let stderr = String::from_utf8_lossy(&output.stderr);
        
        // Verify compilation succeeds
        assert!(output.status.success(), 
            "cargo check --workspace should succeed. Stderr:\n{}", stderr);
        
        assert!(stderr.contains("Finished `dev` profile"),
            "Cargo should report successful completion. Stderr:\n{}", stderr);
    }

    /// TC-4: Verify downward dependency L5→L4 is allowed
    #[test]
    fn test_downward_dependency_l5_to_l4_allowed() {
        // This test validates that the script correctly allows L5→L4 downward dependencies
        let output = Command::new("powershell")
            .args(&[
                "-NoProfile",
                "-File",
                "scripts/check_dependency_rules.ps1"
            ])
            .current_dir("d:\\Chimera CLI")
            .output()
            .expect("Failed to execute script");

        let stdout = String::from_utf8_lossy(&output.stdout);
        
        // Verify no upward dependency violations are reported for valid downward edges
        assert!(!stdout.contains("[GAP-B] gsoe-evolution"),
            "gsoe-evolution should not have any GAP-B violations (all deps are downward). Output:\n{}", 
            stdout);
    }

    /// TC-5: Verify chimera-mas internal deps <= 16 bound
    #[test]
    fn test_chimera_mas_internal_deps_bound() {
        let output = Command::new("powershell")
            .args(&[
                "-NoProfile",
                "-File",
                "scripts/check_dependency_rules.ps1"
            ])
            .current_dir("d:\\Chimera CLI")
            .output()
            .expect("Failed to execute script");

        let stdout = String::from_utf8_lossy(&output.stdout);
        
        // Verify WI-29 bound is satisfied
        assert!(stdout.contains("[D] chimera-mas internal deps: 12/16 (WI-29 <=16 bound)"),
            "chimera-mas internal deps should be within WI-29 bound. Output:\n{}", stdout);
    }
}
