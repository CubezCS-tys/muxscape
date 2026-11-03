# Security policy

Muxscape handles SSH connections, host keys and (optionally) saved passwords,
so security reports are very welcome.

## Reporting

Please **don't open a public issue** for a vulnerability. Use GitHub's private
vulnerability reporting ("Report a vulnerability" on the Security tab), or
email the maintainer listed on the repository. Include steps to reproduce and
the version you tested. We'll acknowledge within a few days.

## Scope notes

- The desktop app is in scope. `crates/mux-devserver` is a development tool
  and isn't shipped, but reports about it are still useful.
- Known, documented limitations (see the README) aren't vulnerabilities on
  their own, but ideas to close them are welcome.
