# Security

## Reporting a problem

Please report a security problem privately, not in a public issue: open [a private report](https://github.com/chancrisp/kiln/security/advisories/new) (Security > Report a vulnerability). Only the maintainer sees it.

Include what you found, the Kiln version (Settings > About), and the steps or a file that shows it. You'll usually get an answer within a week, and a fixed release as soon as one is ready. Your report stays private until then, and you're credited in the release notes unless you'd rather not be.

## Supported versions

Fixes go into the latest release only. Update with Settings > About > Check for updates, or from the [releases page](https://github.com/chancrisp/kiln/releases).

## What counts

Examples of what to report:

- a file that makes Kiln write outside the folder it should;
- a file that overwrites or deletes something it shouldn't;
- a file that runs code;
- a way for a file, a helper app or the `kiln:` link to make Kiln do something the person didn't ask for;
- a release file whose signature or SHA-256 doesn't match.

A file that only makes one conversion fail, or Kiln close, is a bug: report it with the [bug report](https://github.com/chancrisp/kiln/issues/new?template=bug_report.yml) form.

Problems in FFmpeg, Pandoc, LibreOffice or Calibre themselves belong with those projects. Do tell Kiln too if Kiln makes one reachable.
