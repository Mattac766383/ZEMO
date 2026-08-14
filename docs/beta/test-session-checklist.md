# Test session checklist — maintainer (M17.1 private beta)

One row / section per tester. Use a tester ID only (T1, T2, …).  
Do not store unnecessary personal information. Do not copy document contents.

Distribution: `ZEMO-0.1.0-beta.5-arm64.dmg` (**not** older packs)  
App version: `0.1.0` · pack `0.1.0-beta.5` · architecture: `arm64` (Apple Silicon) · macOS Apply + Undo

## Per tester

### Identity (minimal)

- Tester ID:
- Mac model:
- Architecture (must be Apple Silicon for this build):
- macOS version:
- Date of first session:
- Date of day-7 follow-up:

### Install

- [ ] DMG opened
- [ ] App dragged to Applications
- [ ] First launch succeeded
- [ ] Gatekeeper right-click Open required? Y/N
- [ ] Install success: Y/N
- Notes (hesitation, wording, false steps):

### Onboarding

- [ ] Welcome appeared on first launch
- [ ] Chose **Organiser mon ordinateur** or **Choisir des dossiers**
- [ ] Permission explanation seen before scope / folder choice
- [ ] Privacy / “scan does not move files” understood? Y/N / unclear
- [ ] Whole Computer = user folders (not system / Applications)? understood? Y/N / unclear
- [ ] macOS permission prompts only after scope choice? Y/N / none observed
- Onboarding success: Y/N

### Corpus

- Number of test files (approx.):
- Folder type (personal copy / synthetic / other):

### Scan

- [ ] Scan started
- [ ] Scan completed
- Duration (if noted):
- Failures / stuck states:
- Scan success: Y/N

### Analysis / proposal / review

- [ ] Document analysis ran (or user reached proposal another way)
- [ ] Organization proposal generated
- [ ] Current vs proposed paths understood? Y/N / unclear
- [ ] TO_REVIEW inspected
- Review burden (light / acceptable / heavy):
- Clearly wrong proposals (count or note):

### Search

- Three search tasks (short labels only):
  1.
  2.
  3.
- Lexical tasks passed: _ / 3
- Semantic model installed? Y/N
- Natural-language tasks passed (if model installed): _ / 3

### Monitoring

- [ ] New file added to watched test folder
- [ ] Detection observed
- [ ] Proposal updated (no Apply)
- Monitoring result: useful / weak / not observed
- Filesystem mutation observed? (must be **none**):

### Trust / confusion

- Thought files had already moved? Y/N
- Felt safe? Y/N / mixed
- Most confusing part:
- Crashes / freezes:

### macOS Apply gate

- [ ] Execution / Apply remains unavailable
- [ ] No unexpected mutation of the test folder

### Day 7

- Reopened? Y/N
- Why / why not:
- Would keep installed? Y/N

### Maintainer follow-up

- Diagnostics received (error excerpt only)? Y/N
- Sensitive paths/content in logs? Y/N (flag if yes)
- Action items:
