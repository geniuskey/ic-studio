# Design report

An example IC Studio extension in Python. It adds the tool `design-report__report` and the
menu command **Extensions > Design report > Design report of the active cell**, which counts
the devices of the cell's schematic, runs DRC and LVS on its layout and writes
`reports/<cell>.md` in the project folder.

```sh
ics ext install https://github.com/geniuskey/ic-studio --path extensions/design-report
# or, from a checkout:
ics ext install extensions/design-report
ics ext call myproj design-report__report '{"cell": "nand2"}'
```

Needs Python 3.8 or newer (`python3`, or `python` on Windows).
