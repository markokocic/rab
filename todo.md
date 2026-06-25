# Todo

## Active / Pi alignment

- [ ] **Write tool output:** Lines don't match screen width, styling/wrapping differ from pi. Needs 1:1 alignment.
- [ ] write tool output doesn't match 1/1 pi. the lines don't have screen width. styling, wrapping. compare with pi and make it 1/1 like it.
- [ ] edit tool diff view, for unchanged lines it should indent by one blank, to align to + or - the other lines have.
- [ ] **Welcome message:** Doesn't look 1:1 identical with pi.
- [ ] **check footer to match 1/1 pi.** check what is missing.
- [ ] **HTML export** — export session to HTML with tool result rendering (pi has `exportSessionToHtml`)
- [ ] **--export <file>** — stub only, not implemented yet
- [ ] **cross-project session forking prompt** — shows warning, no interactive prompt yet (pi prompts y/n)


## Next

- [ ] is the handling of session correct? does it match pi 1/1? is saving and loading of session data handled correctly in all edge cases?

- [ ] in markdown renderer there is a gap. I see sometimes the following:
  ```markdown
  
  markdown content
  ```
  It should not be rendered as source, but as a regular markdown? How does Pi handles this case?

- [ ] Check if there's stripping of newlines in markdown display components

- [ ] from time to time I get the following message: "No response from provider after 15s — connection may be stalled". check how pi handles that? we need to close the gap and do 1/1 what pi does.

## Todos

- [ ] bring back kitty and image support. should work 1/1 like pi. crossterm should support it.
- [ ] all bash tools show 1.0s. looks like duration is not properly updated
- [ ] welcome message doesn't look 1/1 identical with pi
