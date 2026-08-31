# Cursor — how does a `hooks.json` `command` string become argv?

**Question.** Does Cursor apply shell word-splitting, quoting, variable expansion, and command substitution to a `hooks.json` `command` string before spawning the hook process — or does it split the string some other way, or not at all?

**Why it matters.** agentspec already composes a multi-word command string. `hook_command_anchor` (`src/adapters/hook_compile.rs:256-281`) emits `{env_assignment}{shim_path} {user_script_path} {hook_id}`, where `env_assignment` is a `VAR=value ` prefix under `HookEmitMode::MergedUser` and `HookEmitMode::MergedProject`. A leading assignment is shell syntax with no meaning to a non-shell executor, so that prefix is shipped behavior resting on an assumption nothing has measured. The `envprefix` case is the one that verifies it.

Cursor's documentation defines `command` as "a shell string, an absolute path, or a relative path" and names no interpreter, no `args` field, and no escaping rules. Claude Code, by contrast, documents `sh -c` for shell form and an exec-form `args` array that bypasses quoting entirely — so agentspec has a grounded basis on Claude and none on Cursor. The `args` feature this de-risks is tracked separately in `$THOUGHTS_DIR/ideas/2026-08-31-agentspec-hook-entry-args.md`.

**Driver:** `manual`. A person must drive Cursor for the probe to run. The answer is counted from the capture by a projection, so no person interprets it.

## Running it

```sh
experiments/cursor-hook-command-argv/probe.sh
```

1. Open Cursor on the printed workspace
2. Fully quit and reopen Cursor — it reads `.cursor/hooks.json` at start
3. Start a **fresh** conversation — not a resume; Cursor's `sessionStart` fires only on initial conversation creation, which `cursor-session-start` measured

One session exercises the whole matrix. `.cursor/hooks.json` registers eleven `sessionStart` entries, all reaching the same capture script with different argument suffixes, and each appends one line recording its own `$0`, `$#`, and positionals alongside Cursor's untouched payload.

## The assertion, key by key

```json
"expected": {
  "controls": true, "argv0": true, "split": true, "dquote": true, "squote": true,
  "var": true, "subst": true, "meta": true, "utf8": true,
  "envprefix": true, "envprefix_control": true, "anchor": true
}
```

- **`controls`** — at least two entries reached the capture script with zero arguments. These are the two bare-executable-path registrations that bracket the nine test cases in the array; see "the controls" below. The threshold is `>= 2` rather than exactly 2 deliberately: `wait_for` releases at two, so a third zero-arg firing landing between the release and the slurp — a second window, a re-fire, or the entry-ordering non-determinism conceded under "Oracle limits" — would otherwise record `refuted` for an apparatus reason while every substantive key read `true`. `refuted` is the loud signal, and spending it on that would be a false alarm.
- **`argv0`** — `$0` ends in `dump-hook.sh`. This is the most direct discriminator between execution models: `sh -c "script args"` sets `$0` to the script path, while some wrappers set it to `sh` and a direct exec may set it otherwise.
- **`split`** — `{{CAPTURE_SCRIPT}} split alpha beta` arrived as three positionals. Whitespace was split on at all.
- **`dquote`** / **`squote`** — `"two words"` and `'two words'` each arrived as one positional with the quotes removed. Quotes were both honored and stripped.
- **`var`** — `$HOME` was expanded: `argv[1]` is not the literal `"$HOME"` and starts with `/`. Read a `false` here as weaker than the others: it holds both when no expansion occurred _and_ when a shell expanded an unset or empty `HOME`, which drops the positional entirely and leaves `argc == 1`. `subst` covers expansion independently, so a `var: false` beside a `subst: true` is an apparatus artifact rather than a finding.
- **`subst`** — `$(echo SUBSTITUTED)` was executed and replaced by its output.
- **`meta`** — `'a;b|c&d'` arrived as one literal positional. Metacharacters inside quotes were not acted on.
- **`utf8`** — `'café-日本-ñ'` arrived byte-intact.
- **`envprefix`** — a leading `AGENTSPEC_PROBE_ENV=probe-value` was honored as an environment assignment, so the hook process saw it in its environment. This is the key that tests shipped `hook_command_anchor` behavior.
- **`envprefix_control`** — the `split` case, which carries no assignment prefix, saw `AGENTSPEC_PROBE_ENV` _unset_. Without this, `envprefix` would read `true` if the variable merely happened to be exported in the environment Cursor inherited, independently of whether Cursor honored the prefix at all. Since `envprefix` is the one key testing shipped behavior, a false confirm there is the most expensive thing this projection could produce, and the control costs nothing — the `split` case is already in every capture where `envprefix` could be read.
- **`anchor`** — the shape agentspec actually emits, with a quoted argument containing spaces, arrived as four correct positionals.

**Why every key is a boolean rather than raw argv.** Three cases have machine-specific correct answers — `$HOME` expands to the operator's home directory, `$0` is a temp workspace path, and the capture script's own path varies per run. A projection emitting raw argv would need an `expected` that differs per operator, which no committed manifest can carry. Each case therefore asserts the _property_ instead.

This also satisfies the contract's rule against asserting on absence. A case that never fired projects `false`, because the reduce leaves its key absent and `null.argv` compares unequal. So `true` is positive proof that the case fired **and** produced the expected argv — never a reading of silence.

**`expected` predicts `sh -c` semantics.** That is the prediction the evidence supported when the manifest was authored — lefthook runs `sh -c`, and Claude Code documents `sh -c` for shell form — and it is the prediction agentspec's shipped `env_assignment` already depends on. A refutation would have been the finding, not a failure.

**It was confirmed.** Cursor **3.17.21** produced all twelve keys `true` on the first live run: full word-splitting, both quote forms honored and stripped, `$HOME` expanded, `$(…)` substituted, metacharacters literal inside quotes, UTF-8 intact, and a leading `VAR=value` honored as an environment assignment. `$0` was the capture script's own path rather than `sh`. So Cursor hands the `command` string to a shell with `sh -c` semantics, and **`hook_command_anchor`'s `env_assignment` prefix is measured behavior rather than an assumption** — the specific thing this package was written to settle.

**Read `envprefix: false` precisely.** It means the assignment was not honored, not that the hook did not fire. If Cursor uses a JavaScript `shell-quote` or `execa`-style parser rather than `sh -c`, a leading `VAR=value` becomes argv[0] rather than an assignment — but the case's own line is still in the capture, with `probe_env` empty.

**The likeliest non-`sh -c` outcome is mixed, not wholly false.** A `shell-quote`/`execa`-style parser splits and unquotes without expanding, so it honors `dquote` and `squote` while leaving `var`, `subst`, and `envprefix` false. Judge each key on its own: a partial result is a finding about _which_ parser Cursor uses, not a broken run, and reading a mixed record as "no shell" would lose the answer the session paid for.

## The controls

Two of the eleven entries are controls, registered first and last, reaching the capture script with no arguments at all.

A control must run under _every_ execution model the probe might discover. If Cursor executes the `command` string directly as a path rather than handing it to a shell, then a string with appended arguments resolves to no executable and produces no payload — so a `wait_for` keyed on any test case would poll to its full timeout on a live session and record nothing. A bare executable path is valid under both models, so it always fires.

`wait_for` requires two payloads with `argc == 0`. The design intent was that the array brackets every test case between the two controls, so seeing both would mean every case in between had had its chance to fire — making silence from a middle case evidence about Cursor rather than a race against the poller. **The first live run refuted that**: Cursor does not fire `sessionStart` entries in array order, so the bracket bounds nothing. See "Oracle limits" for the observed ordering and what it costs. The controls still do their primary job — they guarantee that _something_ fires under either execution model, which is what keeps a non-shell result from being an indistinguishable timeout.

**The second control is an exec shim, not a duplicate entry.** Two byte-identical `hooks.json` entries are a dedup hazard: if Cursor collapses them, `wait_for` can never reach two and a good live session polls to full timeout. The second control therefore points at `control-b.sh`, a two-line script whose entire body is `exec "{{CAPTURE_SCRIPT}}"`. The two config entries are textually distinct, so nothing can collapse them, while both remain bare executable paths that survive a non-shell model.

That shim sits at `$ws/project/control-b.sh` rather than in `$ws/capture/`, because the runner's fixture loop skips everything under `capture/` and substitutes `{{CAPTURE_SCRIPT}}` only outside it — a script needing that one placeholder and no other can _only_ live outside `capture/`. The contract's reason for the capture/project split is that a probe whose oracle is the agent's answer can be defeated by the agent finding a planted marker; this probe's oracle is the capture file, and the shim carries no marker to find.

## The assertion discriminates

Validated against three synthetic captures, each a workspace directory holding a `capture/payloads.jsonl`, run through `record.sh --dry-run`:

| Capture | Observed |
| --- | --- |
| A — full `sh -c` semantics | `{"controls":true,"argv0":true,"split":true,"dquote":true,"squote":true,"var":true,"subst":true,"meta":true,"utf8":true,"envprefix":true,"envprefix_control":true,"anchor":true}` |
| B — no shell involved; the two controls only | `{"controls":true,"argv0":false,"split":false,"dquote":false,"squote":false,"var":false,"subst":false,"meta":false,"utf8":false,"envprefix":false,"envprefix_control":false,"anchor":false}` |
| C — naive whitespace splitting, no quote removal, no expansion | `{"controls":true,"argv0":true,"split":true,"dquote":false,"squote":false,"var":false,"subst":false,"meta":false,"utf8":false,"envprefix":false,"envprefix_control":true,"anchor":false}` |

Three distinct values for three distinct inputs. Capture C is the one that matters most: it separates "a shell parsed this" from "something split on whitespace," which a `split`-only matrix could not distinguish. Capture C also models the splitter consistently — with no assignment handling, `AGENTSPEC_PROBE_ENV=probe-value` becomes the command word, which is not an executable, so that case produces **no line at all** and the projection still reads `false`.

## Three deliberate deviations

- **The capture records argv alongside the payload.** The contract's authoring step 3 asks a capture hook to append "the payload as the provider sent it, with nothing added." Argv is what this probe measures and no other channel carries it, so each line wraps the untouched payload under a `payload` key and adds `case`, `argv0`, `argc`, `argv`, and `probe_env` beside it.
- **`version_source` therefore reads `.payload.cursor_version`,** not the top-level field the other Cursor packages use.
- **The source script's outer empty-read guard is dropped.** An empty read now appends `{"case": null, …, "payload": {"unparsed": ""}}` where the source appended nothing, so `record.sh`'s non-empty-capture check can pass on a capture no hook delivered a payload to. Diagnose a bad run by reading `argc` and `payload`, not the line count.

  What is **not** dropped is the source's `select(type == "object")` filter, which the payload expression carries across as `((($raw | fromjson?) | select(type == "object")) // {unparsed: $raw})`. An earlier draft used a bare `fromjson? // {unparsed: $raw}`, on the reasoning that the _line_ stays an object either way — true, but not the property that matters once the payload is nested. A payload that parses as valid non-object JSON (a bare array or scalar) passes `fromjson?` and lands under `.payload` verbatim, and then `version_source`'s `.payload.cursor_version` throws on it. Because `record.sh` evaluates that expression as `jq -s` over the whole file and swallows the error (`2>/dev/null || true`), a single such line would cost the recorded version for the **entire** capture, writing `tool_version: null` and a `…-unknown.json` filename with no diagnostic anywhere.

## Oracle limits

- **Entry ordering is unspecified — and the first live run measured it as non-sequential.** The bracket argument assumed Cursor fires `sessionStart` entries in array order. It does not. In the 3.17.21 capture the lines arrived `meta`, `dquote`, `split`, `envprefix`, `utf8`, `anchor`, `squote`, **control**, `var`, `subst`, **control** — the first control, registered first in the array, landed eighth. So the bracket bounds nothing by construction; the run succeeded because all eleven entries fired within one poll interval, not because the controls fenced them.

  This does not weaken any recorded key — every `true` is still positive proof that the case fired and produced the expected argv, which is what the non-default rule requires. It weakens only the _diagnostic_ the bracket was meant to provide. A capture with two controls and zero cases therefore warrants **one re-run** before being read as "no shell involved," and that advice is now load-bearing rather than precautionary.

- **A workspace path containing a space would silently invalidate every word-splitting case,** since `{{CAPTURE_SCRIPT}} split alpha beta` would then carry a space before the case id. `probe_template_file` guards JSON validity but not whitespace, so the runner asserts on `$TMPDIR` directly and exits 1 before the operator spends a session.
- **The second control is an exec shim at a distinct path.** A `hooks.json` dedup would not defeat it, but a path-resolution failure would. `control-b.sh` is referenced project-relatively because the runner substitutes only `{{CAPTURE_SCRIPT}}`, so the shim's own absolute workspace path is not available to write into `hooks.json` — and Cursor's documentation warns to write `.cursor/hooks/script.sh` rather than `./hooks/script.sh`. Exactly one `argc == 0` line in a timed-out capture means the shim did not resolve; the fix is to move it to `fixtures/.cursor/control-b.sh` and reference it as `.cursor/control-b.sh`, not to change how it is invoked.

## Depth

`depth: null` — a fact about the hook process's argv is off the config-rendering chain rather than a weaker point on it, the same rationale `cursor-session-start` uses.

`$THOUGHTS_DIR/ideas/2026-08-31-agentspec-hook-entry-args.md` argued for treating this as equivalent to `outbound-request` depth, on the grounds that it observes the real executed artifact. This package departs from that deliberately: the depth axis describes how far a value travels along the config-rendering chain toward the model, and hook argv is not a point on that chain at all.

## Version source

`version_source.kind: "capture"`, reading `.payload.cursor_version` — the nested path this package's line shape requires. `just probe-status` therefore computes no version drift for this package, as for every other Cursor one.
