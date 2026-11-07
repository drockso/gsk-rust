# Security policy

The latest release is the supported version. This early project has been tested on one controller; it does not provide a manufacturer compatibility certification.

Use GitHub's **Report a vulnerability** feature for security issues:
https://github.com/drockso/gsk-rust/security/advisories/new

Include the version, affected operation, a minimal reproduction and sanitized packets if relevant. Do not publish production CNC programs or secrets in an issue.

The observed GSK UDP protocol has no authentication or encryption. Use it on a trusted machine network. A successful SHA-256 check establishes content equality; it does not validate machining instructions. The utility never sends commands to execute a CNC program.
