<!-- seo title="튜토리얼: CMOS 인버터 레이아웃, 검증, 기생 성분 추출" description="단계별 튜토리얼: IC Studio에서 CMOS 인버터 회로도로부터 레이아웃을 생성하고 배선한 뒤, DRC와 LVS를 실행하고, 추출한 기생 성분으로 시뮬레이션하고, GDSII로 내보냅니다." -->

# 실습: 인버터 레이아웃

이 튜토리얼에서는 [첫 번째 실습](tutorial-inverter.md)의 인버터를 검증된 레이아웃까지 완성합니다.
회로도에서 레이아웃을 생성하고, 배선하고, DRC와 LVS로 검사하고, 실제 배선의 기생 성분을 넣어
시뮬레이션한 뒤, 팹이 받게 될 GDSII 파일을 씁니다. 먼저 [회로에서 실리콘으로](layout-basics.md)를
읽어 두세요. 약 20분이 걸립니다.

PMOS가 **2 µm**인 `my_inv` 셀에서 시작합니다(4 µm로 바꿔 보았다면 되돌려 두세요). 프로젝트가
없다면 데모를 열고 `cmos_inverter`의 레이아웃을 지운 뒤 사용하세요(`Ctrl+4`로 열고 `Ctrl+A`,
`Delete`).

## 1단계: 레이아웃 생성

1. `my_inv`를 연 상태에서 **Verify > Generate Layout from Schematic**을 선택합니다.
2. 셀에 레이아웃 뷰가 생기고 Layout 워크스페이스가 열립니다. 다음이 보입니다.
   - 위쪽 행에는 PCell로 만든 PMOS, 아래쪽 행에는 NMOS가 있고 게이트가 서로 마주 봅니다.
   - PMOS를 둘러싼 **n-well**과, 행 끝의 바디 **탭**(`vdd`에 연결된 `ntap`, 접지에 연결된
     `ptap`)이 있습니다.
   - 점선 **플라이라인**: 아직 연결해야 할 곳입니다.
3. 콘솔에 일어난 일이 나열됩니다. 트랜지스터 두 개가 배치되었고, 커패시터와 소스는 건너뛰었습니다
   (`no layout generator`). 이들은 테스트벤치의 일부이지 회로의 일부가 아니기 때문입니다.

`F`를 눌러 화면에 맞춥니다. `Ctrl`+휠로 확대·축소하고, 휠이나 마우스 가운데 버튼으로 이동합니다.

<div class="callout">

**배선하기 전에 살펴보세요.** PMOS를 클릭하고 인스펙터를 보면 W와 L이 회로도에서 온 값입니다.
마우스로 옮겨 보면 플라이라인이 따라옵니다. 인스턴스 이름(`MP`, 여러분의 회로도에서는 `M1`)이
회로도의 소자와 연결해 주기 때문입니다. `Ctrl+Z`로 되돌립니다.

</div>

## 2단계: 배선

1. **Verify > Route Flylines**를 선택합니다.
2. 플라이라인이 메탈 배선과 비아로 바뀝니다. 콘솔에 배선된 넷(이 인버터는 넷 4개, 비아 12개)과
   `0 flyline(s) left`가 보고됩니다.

라우터는 소자 단자에 맞춘 그리드 위에서 동작하고 이름 있는 넷(`in`, `out`, `vdd`)에 레이블을
붙이므로, 그려진 결과는 처음부터 DRC 클린이며 LVS 준비가 되어 있습니다.

## 3단계: DRC

1. 오른쪽 패널에서 **Run DRC**를 누릅니다(또는 **Verify > Run DRC**).
2. 결과: **No violations**.

이제 위반이 어떻게 보이는지 알아보기 위해 일부러 룰을 어겨 봅니다.

1. 다른 metal1 도형 가까이에 있는 metal1 배선을 선택하고 `Shift`+화살표 키로 그쪽으로 조금씩
   옮깁니다.
2. 간격이 metal1 간격 230 nm보다 작아지는 순간 **Live DRC**(기본으로 켜짐)가 점선 박스를
   그립니다. 마우스를 올리면 룰이 보입니다.
3. 위반 위에서 `Ctrl+.`(**Quick Fix**)를 누르거나, 그냥 `Ctrl+Z`를 누릅니다.

## 4단계: LVS

1. **Run LVS**를 누릅니다.
2. 결과에 차이 하나가 나열됩니다.

   ```text
   [missing] schematic device CL (capacitor C=20fF) has no layout counterpart
   ```

이것은 예상된 결과입니다. 20 fF 부하는 *다음 게이트*를 모델링한 것이고, 이 셀의 일부가 아닙니다.
LVS는 회로도에 있는 모든 것을 비교하므로 이를 보고합니다. 넷 `in`, `out`, `vdd`, 접지는 모두
일치하고, 두 트랜지스터도 일치합니다.

실제 프로젝트에서는 테스트벤치(소스, 부하)를 인버터를 인스턴스로 *배치하는* 별도의 셀에 두고,
인버터 셀 자체에는 회로만 넣습니다. 그러면 LVS가 완전히 깨끗해집니다. 직접 해 보세요. 회로도에서
`CL`을 지우고 LVS를 다시 실행하면 **Layout matches schematic**이 나옵니다. (계속하기 전에 `Ctrl+Z`로
되돌리세요. 시뮬레이션에는 부하가 필요합니다.)

## 5단계: 포스트 레이아웃 시뮬레이션

1. `Ctrl+3`을 눌러 Simulation 워크스페이스로 갑니다.
2. **Include layout parasitics**를 체크하고 `F5`를 누릅니다.
3. 콘솔에 추출 결과가 요약됩니다. 예를 들면 다음과 같습니다.

   ```text
   post-layout: 3 nets annotated, 2.434 fF to ground, 5 coupling caps (0.547 fF),
   junctions on 2 devices, 40 resistors on 4 nets
   ```

4. [첫 번째 튜토리얼](tutorial-inverter.md#8단계-지연-과도-해석)에서처럼 Waveform Calculator로 지연을 다시 측정합니다.

| | t<sub>pHL</sub> | t<sub>pLH</sub> |
|---|---|---|
| 회로도 | 36.4 ps | 51.8 ps |
| 포스트 레이아웃 | 39.8 ps | 56.1 ps |

레이아웃이 20 fF 부하에 약 3 fF의 배선·접합 커패시턴스와 약간의 배선 저항을 더해, 약 9 %
느려집니다. 인버터 하나에서는 작은 차이지만, 칩을 가로지르는 긴 배선에서는 기생 성분이 지연을
완전히 지배할 수도 있습니다.

**Compare runs**에서 회로도 실행을 체크하면 두 파형을 겹쳐 볼 수 있습니다.

## 6단계: GDSII 내보내기

GDSII는 팹이 받는 파일 형식입니다.

- 레이아웃 툴바에서 **GDS**를 누르고 파일 이름을 고르거나,
- 터미널에서 `ics gds <project> my_inv -o my_inv.gds`를 실행합니다.

파일에는 모든 레이어의 폴리곤이 테크놀로지의 GDS 레이어 번호와 함께 들어 있습니다(`poly`는 5/0,
`metal1`은 7/0 등). KLayout 같은 어떤 레이아웃 뷰어로도 열 수 있습니다.

## 지금까지 한 일

풀 커스텀 설계 루프 전체를 한 바퀴 돌았습니다.

<div class="ics-flow">
<span>회로도</span><i>→</i>
<span>시뮬레이션</span><i>→</i>
<span>레이아웃 생성</span><i>→</i>
<span>배선</span><i>→</i>
<span>DRC · LVS</span><i>→</i>
<span>기생 성분 추출</span><i>→</i>
<span class="loop">다시 시뮬레이션 ↺</span>
</div>

## 연습 문제

1. **손으로 그려 보기.** 레이아웃 뷰만 있는 셀 `inv_hand`를 만들고, Rectangle 도구(`R`)로
   인버터를 그립니다. active, poly, 콘택(Via 도구 `O`), metal1, n-well, 임플란트가 필요합니다.
   데모 `cmos_inverter`의 레이아웃을 본보기로 삼으세요. DRC가 깨끗해질 때까지 실행합니다. 손으로
   레이아웃을 그려 보면 룰이 왜 있는지 알게 됩니다.
2. **NAND2.** NAND2 회로도(직렬 NMOS 두 개, 병렬 PMOS 두 개, [CMOS 로직](cmos-logic.md#nand와-nor) 참고)를
   그린 뒤 레이아웃을 생성하고 배선합니다. 데모의 `nand2`와 비교해 보세요. 생성기가 직렬 NMOS
   사이에서 확산을 공유했나요?
3. **더 큰 회로.** 데모 `full_adder`를 열고 레이아웃을 지운 뒤(`Ctrl+A`, `Delete`) 두 명령을 다시
   실행합니다. 트랜지스터 28개가 배치·배선되고 DRC와 LVS가 깨끗합니다.
4. **명령줄에서.** 같은 흐름을 스크립트로 실행합니다.

   ```sh
   ics route myproj my_inv --place     # generate, route, save
   ics drc myproj my_inv
   ics lvs myproj my_inv
   ics sim myproj my_inv --parasitics
   ics gds myproj my_inv -o my_inv.gds
   ```

다음: 칩 설계의 나머지 절반, [아날로그 빌딩 블록](analog-basics.md).
