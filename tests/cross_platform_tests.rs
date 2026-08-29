//! Cross-platform integration tests.

#[cfg(test)]
mod tests {
    use std::fs;

    /// Verifies that symlinks are detected correctly on platforms that support
    /// unprivileged symlink creation (Linux / macOS).  On Windows the test is
    /// skipped with an explanatory message because creating symlinks requires
    /// either elevated privileges or Developer Mode to be enabled.
    #[test]
    fn test_symlink_detection() {
        #[cfg(windows)]
        {
            println!(
                "SKIP test_symlink_detection: symlink creation requires elevated privileges \
                 or Developer Mode on Windows."
            );
            return;
        }

        #[cfg(not(windows))]
        {
            let dir = std::env::temp_dir().join("mainstay_symlink_test");
            fs::create_dir_all(&dir).expect("failed to create temp dir");

            let target = dir.join("target_file.txt");
            let link = dir.join("link_file.txt");

            fs::write(&target, b"mainstay").expect("failed to write target");
            std::os::unix::fs::symlink(&target, &link).expect("failed to create symlink");

            let meta = fs::symlink_metadata(&link).expect("failed to read symlink metadata");
            assert!(meta.file_type().is_symlink(), "expected a symlink");

            // Cleanup
            let _ = fs::remove_file(&link);
            let _ = fs::remove_file(&target);
            let _ = fs::remove_dir(&dir);
        }
    }
}
