# Test fixtures

Seed corpora for the resilience tests in `src/resilience_tests.rs`. Regenerate with
`python tests/fixtures/fetch_fixtures.py`.

| File | Source | License |
|:--|:--|:--|
| `commonmark-0.31.2.json` | Examples from the [CommonMark Spec 0.31.2](https://spec.commonmark.org/0.31.2/) by John MacFarlane | [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/) |
| `gfm-0.29.json` | Examples from the [GitHub Flavored Markdown Spec 0.29](https://github.github.com/gfm/) (`github/cmark-gfm`, `test/spec.txt`) | [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/) |

Only the Markdown inputs, example numbers and section names are kept. The expected HTML is dropped,
because ƜackDown never produces HTML. These files are test data, not part of the built application.
