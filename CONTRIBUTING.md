# Contributing

EntropiaOrme is a personal-use-first open-core project: built primarily for my own use, with public source under MIT. It's released openly so anyone who finds it useful is free to use, fork, or adapt it. Right now I'm not accepting external contributions.

## Bug reports

Bug reports are welcome via [GitHub issues](https://github.com/entropiaorme/entropiaorme/issues). Helpful to include:

- The app version (Settings, About panel).
- What you expected vs what happened.
- Any relevant log output.

For security-related issues see [SECURITY.md](SECURITY.md); please do not file public issues for those.

## Contributions

Pull requests for new features, refactors, or non-bug-fix changes are on hold for now. This keeps the maintenance surface small enough that the project stays sustainable as a one-person effort.

If you're interested in contributing, please reach out first at MikelWL@protonmail.com with what you have in mind. I'll reconsider the posture if enough interest is expressed.

## Development setup

If you're working on the code (forking or adapting it), the build-from-source steps are in the [README](README.md#build-from-source-windows). After installing the development dependencies, run `pre-commit install` once so the local hooks run the same lint, type, test, and hygiene checks as continuous integration before each commit. See [TESTING.md](TESTING.md#local-checks-pre-commit) for what the hooks cover and how to run them on demand.
