# Change Log

# [0.3.0] - 2020-09-15
This release most importantly fixes error handling, which was partially broken
in the 0.2 release. Users should upgrade.
- Fix error handling (fixes #2, thanks @winstonewert) and improve documentation
- Make flush calls respect the order relative to write calls 
  (5bb26be195bec2f609cf3f7712c6d47e0b19f407)
- More extensive documentation
- The crate now relies on the 2018 Rust edition
- Misc. smaller improvements

# [0.2.0] - 2018-12-25
- Initial release
