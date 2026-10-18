# Contributing

Issues and pull requests are welcome.

## Before you open a pull request

Run the same checks as CI (see [Development](README.md#development) in the README):

```sh
just ci      # fmt check, clippy, tests, cargo-deny
```

Keep changes focused, and add or update tests for behaviour you change. UI changes usually
need their [insta](https://insta.rs) snapshots reviewed and updated.

## AI-assisted contributions

AI tools are fine to use. Please be open about it:

- **Say so in the pull request description**: which tool you used and roughly for what (for
  example "Claude Code wrote the first draft of the parser; I rewrote the error handling").
- **Add a trailer to commits** where an AI tool wrote a meaningful part of the change:

  ```
  Co-Authored-By: Claude <noreply@anthropic.com>
  ```

  Use the name of the tool or model you actually used. Small autocompletions don't need a
  trailer.
- **You're responsible for what you submit.** Read and understand every line, run the
  checks, and be ready to explain or change it in review, the same as code you wrote yourself.

## License

By contributing, you agree that your contributions are licensed under the project's
[CC BY-NC 4.0](LICENSE) license.
