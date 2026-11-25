<!-- seo title="IC Studio 강좌 장 직접 쓰기" description="선생님과 팀은 프로젝트에 Learn 장을 추가할 수 있습니다. 직접 해 보기 버튼, 학생 설계의 실시간 확인 항목, 퀴즈, 슬라이더 그림이 들어간 Markdown 파일입니다." -->

# 나만의 강좌 장 만들기

프로젝트에 자체 장을 넣을 수 있습니다. 수업용 실습이나 팀의 신입 교육 자료로 쓰세요. 프로젝트의
`learn` 폴더에 Markdown 파일을 둡니다(`my_project/learn/lab1.md`, 파일 이름 순서로 읽습니다.
옆에 `lab1.ko.md`를 두면 한국어판입니다). 프로젝트를 열면 Learn 창의 **목차**에서 강좌 다음의
**이 프로젝트** 아래에 나타납니다. 쓰는 동안에는 그곳의 **다시 불러오기**로 다시 읽습니다.

장은 Learn 강좌와 같은 추가 문법을 HTML 주석으로 쓰는 평범한 Markdown입니다(웹 페이지나 Git
호스트에서는 보이지 않습니다).

```text
<!-- try: NAND 게이트 열기 | open nand2 schematic -->
<!-- check: NAND에 트랜지스터 네 개가 있습니다 | nand2 schematic 4 -->
<!-- check: M1이 포화 영역에 있습니다 | my_amp region M1 saturation -->
<!-- check: 충분히 빠릅니다 | my_inv measure tran "delay(v(in), 0.9, v(out), 0.9, rise, fall)" "" 50p -->
<!-- check: DRC 통과 | my_inv drc -->
<!-- explore: mosfet -->
<!-- quiz
? 입력이 high일 때 전류가 흐르는 트랜지스터는?
- PMOS
+ NMOS
> high 게이트는 NMOS를 켜고 PMOS를 끕니다.
-->
```

- **try** 버튼: `demo`, `open <셀> [schematic|simulation|layout]`, `simulate <셀>`,
  `drc <셀>`, `lvs <셀>`, `derive <셀>`, `new-cell`, `calculator`, `tutorial`, `palette`.
- **check** 셀에 대한 조건: `schematic [소자 수]`, `layout`, `simulated`,
  `region <소자> <영역>`, `measure <op|dc|ac|tran> "<식>" <최소> <최대>`(제한이 없으면 빈
  `""`), `drc`, `lvs`. 실시간으로 갱신되고, 실패하면 조언이 붙습니다.
- **quiz**: `+`가 정답 보기입니다. `= 70 ±2 µA`로 쓰면 숫자로 답하는 문제가 됩니다.
- **explore**: 이 강좌의 슬라이더 그림 `mosfet` 또는 `inverter`.

강좌가 열려 있는 동안 탐색기는 그 장에서 쓰는 셀만 보여 줍니다. 학생은 **보고서…**로 진도를
저장해 Markdown 파일로 제출할 수 있습니다.
