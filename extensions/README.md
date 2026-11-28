# IC Studio extensions

This folder holds the **community index** of IC Studio extensions (`index.json`) and two
example extensions maintained with IC Studio:

| Folder | What it shows |
|---|---|
| [`design-report`](design-report) | A Python program: a tool that calls IC Studio back (netlist, DRC, LVS) and writes Markdown, plus a menu command |
| [`metal-frame`](metal-frame) | No program: one SKILL script as a menu command |

How to use and write extensions: [Extensions](https://pages.euiyun.com/ic-studio/extensions.html)
(`docs/extensions.md`).

## How the ecosystem works

- **Extensions live in their authors' repositories.** IC Studio installs them straight from
  git (`ics ext install <url>` or **Extensions > Manage Extensions**). Nothing is uploaded to or
  built by this project.
- **The index is a list, not a store.** `index.json` maps an id to a repository (and optionally
  a sub-folder and a tag). `ics ext search`, `ics ext install <id>` and the Community index in
  the app read it from the `main` branch.
- **Changes to the index go through pull requests** to this repository, reviewed by the
  maintainers like any other change.
- **Authors choose their own license.** Extensions talk to IC Studio over a documented
  protocol (JSON-RPC on stdin / stdout) as separate programs; `ics_extension.py`, the Python
  helper, is MIT licensed so it can be copied into any extension.
- **Anyone can run their own index** (for example inside a company) and point IC Studio at it
  with `ICS_EXTENSION_INDEX=<url or file>`.

## Adding your extension to the index

1. Publish the extension in a public git repository with a README (what it does, what it
   needs, an example) and a license file.
2. Run `ics ext check <folder>` and install it once from the repository URL.
3. Tag a release (`v0.1.0`) and open a pull request adding one entry to `index.json`, keeping
   the list sorted by id:

```json
{
  "id": "my-extension",
  "name": "My extension",
  "description": "One sentence: what it adds.",
  "repository": "https://github.com/you/ics-my-extension",
  "path": "sub-folder (only if extension.toml is not at the root)",
  "rev": "v0.1.0",
  "commit": "the full 40-character hash the tag points at",
  "license": "MIT",
  "authors": ["Your Name"],
  "tags": ["layout", "python"]
}
```

`id` must equal the `id` in `extension.toml` (IC Studio refuses the install otherwise).
`commit` pins the entry: IC Studio checks out exactly that commit, so moving the tag or
pushing to the branch later changes nothing for users until the index changes. Entries in
other repositories than IC Studio's own must have one, and the repository must be `https://`.
To publish a new version, open a pull request that changes `rev` and `commit`.

A pull request that changes only `index.json` needs no CLA ([CLA.md](../CLA.md) covers code and
documentation contributed to IC Studio itself; your extension stays yours, under your license).

### Review checklist

Maintainers check, and authors should check before opening the pull request:

- [ ] The id is free, descriptive and does not imitate another extension or a vendor's
      product name.
- [ ] The repository is public, has a license and a README, `rev` names an existing tag and
      `commit` is the commit that tag points at.
- [ ] `ics ext install <repository> --path <path> --rev <rev>` works and the tools run.
- [ ] Tool descriptions say what each tool needs and returns (AI agents choose tools by them).
- [ ] The program only does what the README says: no network access, telemetry or file access
      outside the project unless the README states it and the user opts in.
- [ ] No bundled binaries without their source, and no foundry PDK data or other files the
      author may not redistribute.

Entries can be removed again when the repository disappears, stops working with current
IC Studio versions, or breaks the rules above.

## Security

An extension with a `[run]` program runs that program with the user's rights. The index lists
extensions; it does not vouch for them. Report a malicious or vulnerable extension by opening
an issue in this repository, and it will be removed from the index.
