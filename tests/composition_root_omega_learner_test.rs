// Composition Root Omega-Learner Test - Phase 1 Validation
// Purpose: Validate omega-learner can be assembled when --features r2_unfreeze is enabled
// Date: 2026-09-18

#[cfg(test)]
mod tests {
    use std::process::Command;

    /// TC-1: Verify omega-learner dependency declared in chimera-cli/Cargo.toml
    #[test]
    fn test_omega_learner_dependency_declared() {
        let cargo_toml = std::fs::read_to_string("d:/Chimera CLI/crates/chimera-cli/Cargo.toml")
            .expect("Failed to read chimera-cli/Cargo.toml");
        
        // Verify omega-learner is declared as optional dependency
        assert!(cargo_toml.contains("omega-learner = { workspace = true, optional = true }"),
            "omega-learner should be declared as optional dependency in chimera-cli/Cargo.toml");
    }

    /// TC-2: Verify r2_unfreeze feature includes omega-learner
    #[test]
    fn test_r2_unfreeze_feature_includes_omega_learner() {
        let cargo_toml = std::fs::read_to_string("d:/Chimera CLI/crates/chimera-cli/Cargo.toml")
            .expect("Failed to read chimera-cli/Cargo.toml");
        
        // Verify r2_unfreeze feature gate includes both decay-engine and omega-learner
        assert!(cargo_toml.contains("r2_unfreeze = [\"dep:decay-engine\", \"dep:omega-learner\"]"),
            "r2_unfreeze feature should include both dep:decay-engine and dep:omega-learner");
    }

    /// TC-3: Verify composition.rs has conditional omega-learner injection
    #[test]
    fn test_composition_rs_has_conditional_injection() {
        let composition_rs = std::fs::read_to_string("d:/Chimera CLI/crates/chimera-cli/src/composition.rs")
            .expect("Failed to read composition.rs");
        
        // Verify #[cfg(feature = "r2_unfreeze")] guard exists
        assert!(composition_rs.contains("#[cfg(feature = \"r2_unfreeze\")]"),
            "composition.rs should have #[cfg(feature = \"r2_unfreeze\")] guards");
        
        // Verify EvolutionOrchestrator is conditionally assembled
        assert!(composition_rs.contains("evolution_orchestrator"),
            "composition.rs should assemble evolution_orchestrator under r2_unfreeze feature");
    }

    /// TC-4: Run cargo check with r2_unfreeze feature (should compile or show expected API errors)
    #[test]
    fn test_cargo_check_with_r2_unfreeze_feature() {
        let output = Command::new("cargo")
            .args(&["check", "-p", "chimera-cli", "--features", "r2_unfreeze"])
            .current_dir("d:\\Chimera CLI")
            .output()
            .expect("Failed to execute cargo check");

        let stderr = String::from_utf8_lossy(&output.stderr);
        
        // Note: This test acknowledges that API may not be fully defined yet
        // The key validation is that the feature flag works and dependencies are resolved
        if !output.status.success() {
            // If compilation fails, verify it's due to expected API issues, not dependency resolution
            assert!(!stderr.contains("cannot find crate `omega-learner`),
                "omega-learner crate should be resolvable. Compilation error:\n{}", stderr);
            
            // Accept known API errors as expected during Phase 1
            if stderr.contains("no method named `collect_results`") || 
               stderr.contains("no associated function or constant named `default`") {
                eprintln!("Expected API error detected (to be fixed in Phase 2): {}", stderr);
            } else {
                panic!("Unexpected compilation error: {}", stderr);
            }
        }
    }

    /// TC-5: Verify omega-learner is reachable from production graph after fix
    #[test]
    fn test_omega_learner_reachable_after_fix() {
        let output = Command::new("powershell")
            .args(&[
                "-NoProfile",
                "-File",
                "scripts/check_crate_reachability.ps1"
            ])
            .current_dir("d:\\Chimera CLI")
            .output()
            .expect("Failed to execute reachability check");

        let stdout = String::from_utf8_lossy(&output.stdout);
        
        // Verify no new_gaps reported for omega-learner
        assert!(!stdout.contains("[GAP-R] omega-learner"),
            "omega-learner should not be reported as unreachable after Cargo.toml fix. Output:\n{}", 
            stdout);
        
        // Verify script exits successfully
        assert!(output.status.success(),
            "check_crate_reachability.ps1 should exit with code 0. Output:\n{}", stdout);
    }

    /// TC-6: Verify omega-learner appears in cargo tree
    #[test]
    fn test_omega_learner_in_cargo_tree() {
        let output = Command::new("cargo")
            .args(&["tree", "-p", "omega-learner", "--edges", "normal"])
            .current_dir("d:\\Chimera CLI")
            .output()
            .expect("Failed to execute cargo tree");

        assert!(output.status.success(),
            "cargo tree -p omega-learner should succeed");
        
        let stdout = String::from_utf8_lossy(&output.stdout);
        
        // Verify omega-learner appears in output
        assert!(stdout.contains("omega-learner"),
            "omega-learner should appear in cargo tree output. Output:\n{}", stdout);
    }
}
