<!-- seo title="IC 설계 용어집: 용어 풀이" description="입문자를 위한 집적 회로 설계 용어집: 액티브와 바이어스부터 테이프아웃, 문턱 전압, 웰 탭까지, 더 공부할 책 목록과 함께." -->

# 용어집

이 과정과 가이드의 나머지 부분에서 쓰는 용어를 영어 알파벳 순으로 정리했습니다.

| 용어 | 뜻 |
|---|---|
| **AC analysis** (AC 해석) | 주파수에 따른 소신호 시뮬레이션: 입력에 1 V 사인파를 넣었을 때 모든 노드의 이득과 위상. 보드 선도를 얻습니다. |
| **Active** (액티브, diffusion, OD) | 트랜지스터와 탭을 위해 실리콘이 드러나 있는 레이아웃 레이어. 트랜지스터는 폴리가 액티브를 가로지르는 곳입니다. |
| **Bias** (바이어스) | 트랜지스터의 동작 위치를 정하는 DC 전압과 전류. |
| **Bode plot** (보드 선도) | 로그 주파수 축에 그린 이득(dB)과 위상(°). |
| **Body** (바디, bulk, B) | MOSFET의 네 번째 단자: 채널 아래의 실리콘. NMOS는 기판(접지), PMOS는 n-well(V<sub>DD</sub>)입니다. |
| **BSIM4** | 대부분의 파운드리 PDK가 쓰는 업계 표준 MOSFET 컴팩트 모델. |
| **Channel** (채널) | 트랜지스터가 켜졌을 때 전류가 흐르는, 게이트 아래의 얇은 반전층. |
| **Channel-length modulation** (채널 길이 변조, λ) | 포화 영역에서 V<sub>DS</sub>에 따라 드레인 전류가 조금씩 늘어나는 현상. r<sub>o</sub>를 정합니다. |
| **CMOS** | Complementary MOS(상보형 MOS): NMOS 풀다운 네트워크와 PMOS 풀업 네트워크로 만든 로직. |
| **Common centroid** (공통 중심) | 매칭 소자를 끼워 넣듯 배치(`ABBA`)해 중심을 같게 만들어 공정 기울기를 상쇄하는 방법. |
| **Contact** (콘택) | metal1을 액티브나 폴리에 연결하는, 금속으로 채운 구멍. |
| **Corner** (코너) | 공정 극단값의 모델 세트: `tt`(typical), `ff`(fast), `ss`(slow), `fs`, `sf`. |
| **dB** (데시벨) | 전압 비의 20·log₁₀. ×10 = 20 dB, ×0.707 = −3 dB. |
| **dbu** | 레이아웃 좌표의 데이터베이스 단위. 범용 테크놀로지에서 1 dbu = 1 nm. |
| **DC sweep** (DC 스윕) | 한 소스를 단계적으로 바꾸며 구한 일련의 동작점. |
| **Design rules** (디자인 룰) | 팹의 기하학적 한계: 폭, 간격, 감쌈, 연장, 면적, 밀도. |
| **Die** (다이) | 웨이퍼에서 잘라 낸 칩 하나. |
| **Differential pair** (차동 쌍) | 꼬리 전류를 공유하는 매칭된 트랜지스터 두 개. 두 입력의 차이를 증폭합니다. |
| **Doping** (도핑) | 실리콘에 불순물을 넣는 것: n형(여분의 전자) 또는 p형(정공). |
| **DRC** | Design Rule Check(디자인 룰 검사): 레이아웃이 디자인 룰을 지키는지 검증합니다. |
| **Drain, source** (드레인, 소스) | 채널의 양 끝. 캐리어는 소스에서 드레인으로 흐릅니다. |
| **Dummy** (더미) | 매칭된 행의 끝에 놓는 쓰이지 않는 소자. 모든 실제 소자가 같은 주변 환경을 갖게 합니다. |
| **Fab** (팹, foundry 파운드리) | 칩을 제조하는 공장. |
| **Finger** (핑거) | 함께 넓은 트랜지스터를 이루는 여러 개의 병렬 좁은 게이트 중 하나. |
| **Flyline** (플라이라인) | 레이아웃에서 아직 배선해야 할 연결을 보여 주는 직선 점선. |
| **g<sub>m</sub>** (트랜스컨덕턴스) | 게이트 전압 1 V당 드레인 전류의 변화: 2·I<sub>D</sub>/V<sub>ov</sub>. |
| **g<sub>ds</sub>**, **r<sub>o</sub>** (출력 컨덕턴스, 출력 저항) | 포화 영역 트랜지스터의 출력 컨덕턴스와 저항. g<sub>ds</sub> ≈ λ·I<sub>D</sub>. |
| **GDSII** | 팹에 보내는 레이아웃의 바이너리 파일 형식(OASIS는 더 새롭고 작은 후속 형식입니다). |
| **Gate** (게이트) | 게이트 산화막으로 채널과 절연된 MOSFET의 제어 단자. 또는 로직 게이트(NAND, NOR …). |
| **Gate oxide** (게이트 산화막) | 게이트와 채널 사이의 얇은 절연막(수 nm의 SiO₂). |
| **Ground** (접지) | 0 V 기준 넷. SPICE에서는 `0`. |
| **Guard ring** (가드 링) | 소자나 블록 주위를 두르는 탭의 고리. 떠도는 캐리어를 모읍니다. |
| **Inverter** (인버터) | NOT 게이트: PMOS 하나와 NMOS 하나. |
| **KCL / KVL** | 키르히호프의 전류 법칙과 전압 법칙. 시뮬레이터가 푸는 방정식입니다. |
| **Latch-up** (래치업) | 전원을 단락시킬 수 있는 CMOS의 기생 사이리스터. 탭과 가드 링으로 막습니다. |
| **Layout** (레이아웃) | 회로의 물리적 도면: 마스크 레이어 위의 폴리곤. |
| **LVS** | Layout Versus Schematic: 레이아웃이 회로도와 같은 회로인지 검증합니다. |
| **Mask** (마스크) | 리소그래피 한 단계의 스텐실. 레이아웃 레이어 하나에 해당합니다. |
| **Matching** (매칭) | 같아야 하는 소자를 크기와 레이아웃으로 최대한 똑같이 만드는 것. |
| **Metal1, metal2 …** (메탈) | 트랜지스터 위에 쌓인 배선 레이어. 비아로 연결됩니다. |
| **Monte Carlo** (몬테카를로) | 무작위 공정 변동을 넣은 여러 번의 시뮬레이션으로 결과의 산포를 보는 방법. |
| **MOSFET** | Metal-oxide-semiconductor field-effect transistor(금속-산화막-반도체 전계 효과 트랜지스터). |
| **Net** (넷, node 노드) | 같은 전압에 있는, 서로 연결된 배선과 핀의 집합. |
| **Netlist** (넷리스트) | 회로의 텍스트 설명: 소자마다 한 줄씩 넷과 값을 나열합니다. |
| **NMOS / PMOS** | n형(전자) 또는 p형(정공) 채널을 가진 MOSFET. |
| **Noise margin** (잡음 여유) | 로직 입력이 잘못 읽히기 전까지 견딜 수 있는 잡음의 크기. |
| **N-well** (n-웰) | PMOS 트랜지스터를 담는, p형 기판 안의 n형 영역. |
| **Operating point** (동작점, OP) | 모든 입력을 고정했을 때 회로의 DC 해. |
| **OTA** | Operational transconductance amplifier(연산 트랜스컨덕턴스 증폭기): 차동 입력, 고임피던스 출력. |
| **Overdrive** (오버드라이브, V<sub>ov</sub>) | V<sub>GS</sub> − V<sub>th</sub>. |
| **Parasitics** (기생 성분) | 실제 배선과 접합이 가진 원치 않는 저항과 커패시턴스. |
| **PCell** | Parameterised cell(파라미터화된 셀): W, L 같은 파라미터로 소자를 그리는 레이아웃 생성기. |
| **PDK** | Process Design Kit(공정 설계 키트): 팹 공정의 레이어, 룰, 소자 모델, PCell. |
| **PEX** | Parasitic extraction(기생 성분 추출): 레이아웃에서 기생 성분을 계산하는 것. |
| **Pelgrom's law** (Pelgrom의 법칙) | 무작위 미스매치는 1/√(W·L)에 비례해 줄어듭니다. |
| **Poly** (폴리, polysilicon 폴리실리콘) | 게이트 레이어. |
| **Post-layout simulation** (포스트 레이아웃 시뮬레이션) | 추출한 기생 성분을 포함한 시뮬레이션. |
| **Propagation delay** (전파 지연, t<sub>p</sub>) | 입력이 50 %를 지난 시점부터 출력이 50 %를 지나는 시점까지의 시간. |
| **Saturation** (포화 영역) | V<sub>DS</sub> ≥ V<sub>ov</sub>이고 전류가 주로 V<sub>GS</sub>에 따라 정해지는 MOSFET 영역. |
| **Schematic** (회로도) | 기호와 배선으로 그린 회로. |
| **Square law** (제곱 법칙) | 포화 영역에서 I<sub>D</sub> = ½·k'·(W/L)·V<sub>ov</sub>². |
| **SPICE** | 표준 회로 시뮬레이터 계열과 그 넷리스트 형식. |
| **Substrate** (기판) | 모든 소자 아래의 웨이퍼 실리콘. 범용 공정에서는 p형입니다. |
| **Supply** (전원, V<sub>DD</sub>) | 양의 전원 넷. 범용 테크놀로지에서 1.8 V. |
| **Tap** (탭) | 트랜지스터 바디를 해당 전원에 묶어 주는 기판 또는 웰 콘택. |
| **Tape-out** (테이프아웃) | 최종 레이아웃을 팹에 보내는 것. |
| **Testbench** (테스트벤치) | 시뮬레이션에서 회로를 동작시키기 위해 주변에 두는 소스, 부하, 해석. |
| **Threshold voltage** (문턱 전압, V<sub>th</sub>) | 채널이 생기는 게이트 전압. 범용 NMOS는 0.45 V. |
| **Transient analysis** (과도 해석) | 시간에 따른 시뮬레이션. |
| **Triode** (트라이오드 영역, linear 선형 영역) | V<sub>DS</sub> < V<sub>ov</sub>인 MOSFET 영역. 채널이 저항처럼 동작합니다. |
| **Unity-gain frequency** (단위 이득 주파수) | 이득이 1(0 dB)로 떨어지는 주파수. |
| **Via** (비아) | 두 메탈 레이어를 연결하는, 금속으로 채운 구멍. |
| **VTC** | Voltage transfer curve(전압 전달 곡선): 게이트의 입력에 대한 출력. |
| **W, L** | 채널 폭과 길이. 설계자가 고르는 크기입니다. |
| **Wafer** (웨이퍼) | 칩을 만드는, 지름 최대 300 mm의 실리콘 원판. |

<!-- quiz
? 전압 이득 1000은 몇 dB일까요?
= 60 ±0.5 dB
> 20 · log₁₀(1000) = 60 dB입니다. 10배마다 20 dB가 더해집니다.
-->

<!-- quiz
? 탭(tap)은 무슨 일을 할까요?
- metal1과 metal2를 연결한다
+ 기판이나 웰을 전원에 묶어 트랜지스터 바디를 바이어스한다
- 시뮬레이션에서 전압을 측정한다
- 넓은 트랜지스터를 핑거로 나눈다
> 탭은 기판(접지)이나 n-well(V<sub>DD</sub>)로 가는 콘택입니다. 탭과 가드 링은 래치업도 막습니다. 금속 레이어끼리는 비아로 연결합니다.
-->

<!-- quiz
? 펠그롬 법칙에 따르면 게이트 면적이 네 배인 트랜지스터의 문턱 전압 미스매치는 …
- 4분의 1
+ 절반
- 그대로
- 두 배
> 미스매치는 1/√(W·L)로 줄어듭니다. √4 = 2이므로 σ는 절반이 됩니다.
-->

<!-- quiz
? PCell은 무엇일까요?
- PDK의 메모리 셀
+ W, L 같은 파라미터로 소자를 그리는 레이아웃 생성기
- LVS를 통과한 셀
- 전원 레일용 파워 셀
> 파라미터화된 셀입니다. W나 L을 바꾸면 레이아웃이 다시 그려집니다. 회로도 기반 레이아웃은 회로도의 소자마다 PCell을 배치합니다.
-->

## 더 읽을거리

- Behzad Razavi, *Design of Analog CMOS Integrated Circuits* — 아날로그 설계의 표준 교재로, 아주
  읽기 쉽습니다(국내 번역본도 있습니다).
- Neil Weste and David Harris, *CMOS VLSI Design: A Circuits and Systems Perspective* — 디지털
  CMOS, 레이아웃, 디자인 룰.
- Jan Rabaey, Anantha Chandrakasan and Borivoje Nikolić, *Digital Integrated Circuits* — 인버터,
  게이트, 지연, 전력을 깊이 있게 다룹니다.
- Tony Chan Carusone, David Johns and Ken Martin, *Analog Integrated Circuit Design*.
- R. Jacob Baker, *CMOS: Circuit Design, Layout, and Simulation* — 실용적인 레이아웃 예제가 많습니다.
- 실제 공정으로 연습할 수 있는 오픈 PDK: SKY130(SkyWater 130 nm)과 IHP SG13G2. 둘 다 IC Studio가
  지원합니다([테크놀로지](../technology.md) 참고).
