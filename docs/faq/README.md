# FAQ maintenance guide

The Mainstay FAQ is generated from one source file, so the Markdown and HTML
versions never disagree.

| File | Role |
|---|---|
| [`faq.json`](faq.json) | Source. Edit this file. |
| [`../faq.md`](../faq.md) | Generated Markdown, grouped by topic. |
| [`index.html`](index.html) | Generated searchable page with feedback buttons. |
| [`../../scripts/generate_faq.py`](../../scripts/generate_faq.py) | Generator and validator. |

## Adding or editing a question

1. Edit `faq.json`. Each question needs an `id` in kebab-case, a `topic` that
   exists in `topics`, one or more `answer` paragraphs, a few `tags` that
   people would search for, and `links` to the documents that hold the detail.
2. Regenerate the outputs:

   ```bash
   python3 scripts/generate_faq.py
   ```

3. Commit the source and both generated files together.

The generator fails if an ID is duplicated, a topic is unknown, an answer is
empty, or a link points to a file that does not exist. CI runs
`python3 scripts/generate_faq.py --check`, which fails if the generated files
are out of date.

Keep answers short. Put the detail in the linked document, and keep the answer
accurate for the current code. When a contract changes, check the answers that
name a function, a default, or a limit.

## Where questions come from

The first set of questions was drafted from the repository documentation. It
was not drawn from support tickets or forum threads, because those were not
available when the FAQ was written. Replace or add questions as real ones
arrive.

To collect new questions, open a GitHub issue with the **FAQ question**
template. Maintainers triage the issues, answer them, and add the useful
answers to `faq.json`.

## Search

The page searches the question text, tags, and answers. Every word in the query
must match somewhere in a question. Matches in the question rank above matches
in the tags, and tag matches rank above matches in the answer.

## Voting and feedback

The page has "Was this helpful?" buttons. A vote is stored in the browser's
local storage, so one browser can vote once per question.

Aggregate votes and zero-result searches need a server to receive and store
them. This repository does not include that server. To collect them, set
`FEEDBACK_URL` to an HTTPS endpoint that accepts JSON POSTs. The constant is
in the page template in `scripts/generate_faq.py`, so set it there and then
regenerate. Do not edit `index.html` directly, because regeneration overwrites it.
The page then sends two kinds of event:

```json
{ "page": "faq", "event": "vote", "question_id": "run-tests", "helpful": true }
{ "page": "faq", "event": "no_results", "query": "how do I ..." }
```

Before you set `FEEDBACK_URL`:

- Search queries are sent as typed, truncated to 200 characters. Confirm that
  your endpoint's retention and access rules are acceptable for that data.
- Do not add a form field or any other way for users to send personal data
  through the search box or the feedback path.
- Setting the value changes the generated page, so commit the regenerated
  `index.html` with it and have it reviewed like any other change.

## Not implemented

- **Automatic FAQ updates from search queries.** Zero-result queries are sent
  to `FEEDBACK_URL` when it is set. Nothing turns them into new questions. A
  maintainer reviews them and writes the answers.
- **Shared vote counts.** Without a configured endpoint, votes stay in the
  browser that cast them, and the page does not show totals.
