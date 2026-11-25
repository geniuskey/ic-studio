<!-- seo title="Write your own IC Studio course chapters" description="Teachers and teams can add Learn chapters to a project: Markdown files with Try it buttons, live checks on the student's design, quizzes and slider figures." -->

# Your own course chapters

A project can carry its own chapters: a lab for a class, or a team's onboarding. Put
Markdown files in the project's `learn` folder (`my_project/learn/lab1.md`, read in file
name order; `lab1.ko.md` next to it is the Korean version). When the project opens they
appear in the Learn window's **Contents** under **This project**, after the course;
**Reload** there reads them again while you write.

The chapters are plain Markdown with the same additions as the Learn course, written as HTML
comments (a web page or Git host does not show them):

```text
<!-- try: Open the NAND gate | open nand2 schematic -->
<!-- check: The NAND has its four transistors | nand2 schematic 4 -->
<!-- check: M1 is in saturation | my_amp region M1 saturation -->
<!-- check: Fast enough | my_inv measure tran "delay(v(in), 0.9, v(out), 0.9, rise, fall)" "" 50p -->
<!-- check: DRC clean | my_inv drc -->
<!-- explore: mosfet -->
<!-- quiz
? Which transistor conducts when the input is high?
- The PMOS
+ The NMOS
> A high gate turns the NMOS on and the PMOS off.
-->
```

- **try** buttons: `demo`, `open <cell> [schematic|simulation|layout]`, `simulate <cell>`,
  `drc <cell>`, `lvs <cell>`, `derive <cell>`, `new-cell`, `calculator`, `tutorial`,
  `palette`.
- **check** conditions on a cell: `schematic [devices]`, `layout`, `simulated`,
  `region <device> <region>`, `measure <op|dc|ac|tran> "<expression>" <min> <max>` (an empty
  `""` for no limit), `drc`, `lvs`. They update live, with advice when they fail.
- **quiz**: `+` marks the right option; `= 70 ±2 µA` asks for a number instead.
- **explore**: `mosfet` or `inverter`, the slider figures of this course.

While the course is open the Explorer lists only the cells the chapter uses. Students can
save their progress with **Report…** and hand in the Markdown file.
