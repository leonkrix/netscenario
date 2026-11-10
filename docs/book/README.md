# The guide

The source of the guide (an [mdBook](https://rust-lang.github.io/mdBook/)). Build it with `just book` (the page `docs/book/book/index.html`), check it with `just book-check`.

## Writing a page

A page explains and shows. The exact definition of a key, a field or an option lives in the reference documents of `docs/`, and a page links to it with the full address (`https://github.com/leonkrix/netscenario/blob/main/docs/...`), so there is one place to be right.

Every fenced block is read by `check.py`, which the tests of the CI run (`python/netscenario/tests/test_book.py`); `just book-check` runs it alone. The tags follow the language, separated by commas:

| Block | What is checked |
| --- | --- |
| `yaml,scenario` | A whole scenario: read and checked by the reader of a run. A first line `# file: name.yaml` names it, or `{{#include path}}` of a file does. |
| `yaml,scenario,run` | The same, and it is run. It lasts at most 10 seconds. |
| `python` | It parses. |
| `python,run` | It is run in a directory that holds the scenario files of the page so far. |
| `bash` | A line that starts with `netscenario` names a command that exists and files that are there. |
| any with `file=name` | The text is written to that name, so a scenario can refer to a CSV file or a trace. |

`{{#include path:first:}}` brings in the lines of a file from the repository (mdBook does the same when it builds). A link to `blob/main/...` must name a file that exists. Every page must be in `src/SUMMARY.md`. Output that cannot be checked is a `text` block; make it with a real run.

## Publishing

`.github/workflows/book.yml` builds the book on every pull request that touches it and publishes it to GitHub Pages from `main`. Pages is switched on once, in the settings of the repository: Settings, Pages, Source "GitHub Actions".
