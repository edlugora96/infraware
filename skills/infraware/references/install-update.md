# Install and update

Use this flow when the user wants to install or update the infraware CLI or its skills. Requires Node &gt;=18 and access to the private repo.

## SSH alias

The commands below use an SSH config alias to route through the correct GitHub identity. Inspect your existing `~/.ssh/config` and select the Host entry whose `HostName` resolves to `github.com` and whose key has access to the private repo. Do not create or modify SSH config as part of this workflow. If no existing alias works, ask the user to pick one or provide one before proceeding.

## CLI install

Fresh install (untagged — resolves the default branch head):

```sh
pnpm add -g git+ssh://git@<github-alias>/edlugora96/infraware.git
```

Replace `<github-alias>` with your own SSH config Host alias for github.com whose account has access to the private repo. Access cannot be inferred from `~/.ssh/config` alone — ask the user if uncertain.

## Skills install

Before installing, ask the user whether they want the skill(s) installed locally (project) or globally. The three available skills are: `infraware`, `infraware-coding`, `infraware-audit`.

**Project (local)** — the default scope. Omit `-g`:

```sh
pnpm dlx skills add git@<github-alias>:edlugora96/infraware.git --skill <SKILL_NAME>
```

**Global** — add `-g` / `--global`:

```sh
pnpm dlx skills add git@<github-alias>:edlugora96/infraware.git --skill <SKILL_NAME> -g
```

The skills CLI creates symlinks by default (no `--copy` flag needed). Do not force `-y`; the CLI may auto-select the agents target.

Replace `<github-alias>` with your own SSH config Host alias for github.com. Never edit `~/.ssh/config` as part of this workflow.

## CLI update

```sh
pnpm add -g git+ssh://git@<github-alias>/edlugora96/infraware.git
```

This is identical to the fresh CLI install command.

## Skills update

For this flow, update by rerunning add in the recorded project/global scope; this replaces same-named installed skills.

### Step 1 — discover scope

Run both lists to find where each skill is installed:

```sh
pnpm dlx skills list          # project scope
pnpm dlx skills list --global  # global scope
```

### Step 2 — re-install the target skill(s)

For each skill to update, re-run the install command that matches its scope. The existing installation is overwritten in place.

- **Project-skill only** — project scope (no `-g`):
  ```sh
  pnpm dlx skills add git@<github-alias>:edlugora96/infraware.git --skill <SKILL_NAME>
  ```
- **Global-skill only** — global scope (`-g`):
  ```sh
  pnpm dlx skills add git@[[ORCA_RICH_MD:59cd738a28f6b8b37a082cbaed5f12f9:inline-html:%3Cgithub-alias%3E]]:edlugora96/infraware.git --skill [[ORCA_RICH_MD:59cd738a28f6b8b37a082cbaed5f12f9:inline-html:%3CSKILL_NAME%3E]] -g
  ```
- **Skill found in both scopes** — ask the user whether to replace project, global, or both before modifying.
- **Skill absent in both scopes** — treat as a new install; ask the user for the desired scope.

Do not guess a skill's presence; always run the list commands first. Do not switch scopes silently. Do not remove a skill before reinstalling; the add command overwrites the existing installation.

If the `git+ssh://` form fails, stop and ask the user for the correct SSH alias or URL form before continuing. The SCP shorthand (`git@<alias>:<org>/<repo>.git`) also works with pnpm for skill commands.