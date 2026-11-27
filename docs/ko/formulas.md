<!-- seo title="수식과 모델" description="IC Studio가 계산에 쓰는 식: 내장 SPICE 엔진의 소자 모델과 잡음 모델, 측정 함수, 최적화기, 몬테카를로와 수율, 기생 성분 추출, IR 드롭과 EM, 미스매치, 공정 에뮬레이션, 픽셀." -->

# 수식과 모델

이 페이지는 IC Studio가 보여 주는 숫자 뒤에 있는 식을 정리합니다. 내장 시뮬레이터의 모델, 스펙을
작성하는 측정 함수, 최적화기와 통계, 그리고 레이아웃 쪽 모델(추출, IR 드롭, 공정 에뮬레이션)입니다.
각 절에는 소스 모듈이 적혀 있습니다. 범용 테크놀로지의 계수는 180 nm급의 예시 값이며 파운드리
데이터가 아닙니다. PDK가 이 값을 대신합니다.

Design Copilot은 이 페이지의 요약을 알고 있습니다. 어떤 숫자가 어떻게 나오는지 물으면 여러분의 값을
식에 넣어 계산해 줍니다.

## 숫자와 단위

- SPICE 값은 SI 단위입니다(V, A, F, Ω, s, m). 배율 접미사는 대소문자를 구분하지 않습니다: `t` 1e12,
  `g` 1e9, `meg` 1e6, `k` 1e3, `m` 1e-3, `mil` 25.4e-6, `u`/`µ` 1e-6, `n` 1e-9,
  `p` 1e-12, `f` 1e-15, `a` 1e-18. 그 밖의 뒤따르는 문자는 단위로 보고 무시합니다.
  **SPICE와 마찬가지로 `M`은 밀리입니다.** 메가는 `meg`로 씁니다. 앱의 표시에서는 `M`이
  1e6이므로, `1M` 같은 표시 값을 필드에 그대로 복사하지 마십시오. (`ics-core/src/units.rs`)
- 표시되는 값은 유효 숫자 4자리와 접두어 T, G, M, k, m, µ, n, p, f, a 중 하나로 나타냅니다.
- 레이아웃: 1 dbu = 1 nm(generic180에서 `dbu_per_um` 1000, 그리드 10 dbu)이고, y축은 위쪽을 향합니다.
  회로도 좌표는 그리드 점이고, y축은 아래쪽을 향합니다.
- 넷리스트의 길이는 미터 단위(`w=1u`)이며, `.option scale=1e-6`을 설정한 모델 라이브러리에서는
  µm 단위입니다.

## 내장 시뮬레이터: 소자 모델

내장 엔진은 `ics-sim/src/engine.rs`, `circuit.rs`, `bsim4.rs`에 있습니다.

- V<sub>t</sub> = kT/q이며 회로 온도에서 계산합니다. 온도는 `.temp` 또는 `.option temp`로 정하고,
  기본값은 27 °C입니다.
- 1, 14, 54가 아닌 MOS level은 거부되므로 **Auto**는 ngspice로 전환합니다.
- **지원하지 않는 것:** `.tf`, `.sens`, `.pz`, `.four`, `.meas`(대신 계산기를 사용),
  EXP와 SFFM 소스, Gear 적분, MOS level 2, 3, 8, 49, 전압 또는 온도에 의존하는 C와 L,
  손실 있는 전송선.

**수동 소자와 소스:**

- **저항:** R(T) = R·(1 + tc1·ΔT + tc2·ΔT²)이며, ΔT = T − 27 °C입니다. 잡음은 4kT/R입니다.
- **커패시터:** C, 상수.
- **인덕터:** L이며, 브랜치 행을 가집니다. 상호 인덕턴스는 M = k·√(L1·L2)이며, |k| ≤ 1입니다.
- **소스:**
  - `PULSE(v1 v2 td tr tf pw per)`: tr과 tf의 기본값은 시간 스텝입니다.
  - `SIN(vo va f td θ φ)` = vo + va·e<sup>−θ(t−td)</sup>·sin(2πf(t−td) + φ).
  - `PWL(...)`은 `td=`와 `r=`를 받습니다.
  - `ac mag [phase]`: `ac`만 쓰면 1입니다.
- **제어 소스:** E, F, G, H는 선형입니다. `POLY`와 `VALUE=`는 B 소스가 됩니다.
- **B 소스:** Newton 반복마다 중심 차분으로 선형화합니다.
- **스위치:** 컨덕턴스가 log G에서 tanh를 따라 1/roff와 1/ron 사이를 움직이며, 히스테리시스는
  vh입니다.
- **T 전송선:** 무손실(Z0, Td).

**다이오드**(기본값: is 1e-14, n 1, cjo 0, vj 1, m 0.5, fc 0.5, tt 0, rs 0, bv ∞, ibv 1 mA,
eg 1.11, xti 3):

```
I = is·(exp(V/(n·Vt)) − 1)              linear above 40·n·Vt; breakdown −ibv·exp(−(V+bv)/(n·Vt))
Q_dep = vj·cjo·(1 − (1 − V/vj)^(1−m))/(1 − m)      (SPICE linearisation above fc·vj)
Q_diff = tt·I
is(T) = is·exp( (T/Tnom − 1)·eg/(n·Vt) + (xti/n)·ln(T/Tnom) )
noise: 2q|I| + kf·|I|^af / f
```

**MOSFET level 1**(기본 level). V와 I는 NMOS 기준이며, PMOS는 모든 부호를 뒤집어 계산합니다.
기본값: vto 0, kp는 u0·ε<sub>ox</sub>/tox에서 구함(없으면 2e-5), lambda 0, gamma 0, phi 0.6,
ld 0, nlev 2. W와 L이 없는 인스턴스는 100 µm를 받습니다.

```
Leff = L − 2·ld          β = kp·W/Leff·m
Vth  = vto + γ·( sqrt(φ − Vbs) − sqrt(φ) )         Vov = Vgs − Vth
cutoff     Vov ≤ 0       Id = 0                      (no subthreshold current)
triode     Vds < Vov     Id = β·(Vov·Vds − Vds²/2)·(1 + λ·Vds)
saturation               Id = ½·β·Vov²·(1 + λ·Vds)
gm  = β·Vov·(1 + λ·Vds)  (sat)        gds = λ·½·β·Vov²  (sat)
gmb = gm·γ / (2·sqrt(φ − Vbs))        (used in the solve, not reported)
```

Level 1에는 이동도 저하, 속도 포화, 단채널 효과가 없습니다. 범용 카드는 λ의 길이 의존성을
보완합니다. 넷리스터가 그린 L마다 카드를 하나씩 쓰며, λ = λ<sub>card</sub>·0.5 µm/L입니다.

| generic180 카드 | vto | kp | λ (0.5 µm에서) | γ | φ | tox | cgso = cgdo | cj | cjsw |
|---|---|---|---|---|---|---|---|---|---|
| nmos | 0.45 | 280 µA/V² | 0.08 | 0.4 | 0.8 | 4 nm | 0.3 nF/m | 1 mF/m² | 0.3 nF/m |
| pmos | −0.45 | 70 µA/V² | 0.1 | 0.4 | 0.8 | 4 nm | 0.3 nF/m | 1 mF/m² | 0.3 nF/m |

Level 1 커패시턴스:

- 오버랩은 상수입니다: C<sub>gs</sub> = cgso·W·m, C<sub>gd</sub> = cgdo·W·m.
- 접합은 cj·AD + cjsw·PD(소스도 마찬가지)이며, pb와 fc를 쓰는 SPICE 공핍 모델을 따릅니다.
  회로도의 MOS에 AD/AS/PD/PS가 없으면 시뮬레이션 덱이 PCell에서 추정합니다([레이아웃 기하](#레이아웃-기하) 참고).
- `capmod=0`(기본값)은 Meyer 게이트 커패시턴스를 쓰며, C<sub>ox</sub> =
  ε<sub>ox</sub>/tox·W·L<sub>eff</sub>·m입니다. 포화 영역에서는 C<sub>gs</sub> = ⅔C<sub>ox</sub>,
  C<sub>gd</sub> = 0입니다. 트라이오드 영역에서는 C<sub>gs</sub> = ⅔C<sub>ox</sub>[1 − ((V<sub>gst</sub> −
  V<sub>ds</sub>)/(2V<sub>gst</sub> − V<sub>ds</sub>))²]입니다. Meyer 모델은 전하를 보존하지 않습니다.
- `capmod=1`은 전하를 보존하는 Ward–Dutton 전하를 쓰며, 포화 영역에서 40/60으로 나눕니다.

Level 1 온도: kp(T) = kp·(T/Tnom)<sup>−1.5</sup>이고, vto와 φ는 SPICE의 밴드갭 규칙을 따릅니다.
λ는 스케일링하지 않습니다.

**BSIM4**(`level=14` 또는 `54`)는 ngspice에 구현된 BSIM4.8 식을 따릅니다.

- binning, 단채널·협폭 V<sub>th</sub>, DIBL, V<sub>gsteff</sub>;
- 이동도(mobmod 0–2), 속도 포화, CLM, DIBL, DITS, SCBE 출력 저항;
- Isub, GIDL/GISL, 게이트 터널링;
- capmod 2 전하와 diomod 1 접합.

도함수는 이중수(dual number)로 정확히 계산합니다. rdsmod ≠ 0, rbodymod, rgatemod, NQS, 응력과
WPE(`sa`, `sb`)는 계산하지 않으며, 각각에 대해 경고합니다. 접합 면적과 둘레는 AD/AS/PD/PS에서
가져오고, 없으면 dmcg/dmci에서 가져오는데, 이 둘의 기본값은 0입니다.

**BJT**(Gummel–Poon의 부분 집합; 기본값: is 1e-16, bf 100, br 1, vaf = ikf = ∞):

```
If = is·(exp(Vbe/(nf·Vt)) − 1)     Ir = is·(exp(Vbc/(nr·Vt)) − 1)
qb = q1·½·(1 + sqrt(1 + 4·q2)),  q1 = 1/(1 − Vbc/vaf − Vbe/var),  q2 = If/ikf + Ir/ikr
Ic = (If − Ir)/qb − Ibc            Ib = If/bf + ise·(…) + Ir/br + isc·(…)
```

BJT에는 rb의 전류 의존성, tf 변조, 준포화(quasi-saturation)가 없습니다.

**JFET level 1**(SPICE 관례, ½ 없음): 포화 영역에서
I<sub>D</sub> = β(1 + λV<sub>DS</sub>)(V<sub>GS</sub> − vto)².

## 내장 시뮬레이터: 해석

**MNA.** 미지수는 노드 전압과, V, L, E, H, B(V) 소자의 브랜치 전류입니다. DC, 과도, AC 해석에서
모든 노드와 접지 사이에 gmin(1e-12 S)을 둡니다. `.ic`와 `.nodeset`은 1e10 S로 노드를 고정합니다.

**DC Newton.** 미지수는 다음 조건을 만족하면 수렴한 것으로 봅니다.

```
|Δx| ≤ (vntol or abstol) + reltol·max(|x_new|, |x_old|)
reltol 1e-4, vntol 1 µV, abstol 1 pA (.option reltol=x also sets the transient tolerance)
```

- 회로에 MOS가 하나라도 있으면 노드 스텝을 ±0.5 V로 제한합니다.
- 다이오드와 BJT는 SPICE `pnjlim`을 씁니다.
- 동작점 계산이 실패하면 엔진은 다음을 차례로 시도합니다.
  1. gmin 스테핑(1e-2에서 1e-12 S까지);
  2. 소스 스테핑(k/20).
- DC 스윕은 각 점을 이전 해에서 시작합니다.

**과도 해석:**

- 적분은 사다리꼴(trapezoidal) 방식입니다. 다만 시작할 때, 각 브레이크포인트 뒤, Newton 실패 뒤,
  링잉이 감지될 때는 후진 오일러(backward Euler)를 씁니다. Gear는 없습니다.
- 스텝은 ngspice 방식으로 국소 절단 오차(LTE)로 제어합니다.

```
tol = max(abstol + lte_reltol·|i|, lte_reltol·max(|q|, chgtol)/h)     lte_reltol 1e-3, trtol 7
del = trtol·tol / (factor·|DD|)        factor 1/12 (trap), ½ (BE); DD a divided difference of q
h_LTE = sqrt(del) (trap), del (BE)     next h = min(h_LTE, 2·h)
```

- 최대 스텝은 min(tstep, tstop/100, tmax)입니다.
- Newton이 실패하면 스텝을 8로 나눕니다.
- 채택된 모든 점을 출력하며, 리샘플링하지 않습니다.
- `uic`는 동작점 계산을 건너뜁니다.

**AC**는 동작점에서 선형화합니다. 주파수는 f<sub>k</sub> =
f<sub>start</sub>·b<sup>k/n</sup>(dec 또는 oct, 끝 주파수를 덧붙임)이거나 `lin`입니다.

**잡음**은 수반(adjoint) 방법을 씁니다: S<sub>out</sub> = Σ|z<sub>a</sub> −
z<sub>b</sub>|²·S<sub>i</sub>.

- `onoise_spectrum`은 √S<sub>out</sub>(V/√Hz)입니다.
- `inoise_spectrum`은 입력 소스에서의 √S<sub>out</sub>/|H|입니다. 전류 소스이면 단위가
  A/√Hz입니다.
- 적분한 출력 잡음은 로그에 기록됩니다.

| 소스 | PSD (A²/Hz) |
|---|---|
| 저항 | 4kT/R |
| MOS level 1, 채널 | (8/3)·kT·g<sub>m</sub> |
| MOS level 1 플리커, `nlev=2`(기본값) | kf·g<sub>m</sub>²/(f<sup>af</sup>·W·L<sub>eff</sub>·C<sub>ox</sub>') |
| `nlev=1` | kf·I<sub>D</sub><sup>af</sup>/(f·W·L<sub>eff</sub>·C<sub>ox</sub>') |
| `nlev=0` | kf·I<sub>D</sub><sup>af</sup>/(f·C<sub>ox</sub>'·L<sub>eff</sub>²) |
| 다이오드, BJT | 2q\|I\| + kf\|I\|<sup>af</sup>/f |
| BSIM4 | 열잡음 tnoimod 0 (R<sub>ds</sub>를 포함한 4kT·μ\|Q<sub>inv</sub>\|/L<sub>eff</sub>²); 플리커 fnoimod 1 (통합 noia/noib/noic) 또는 0; 게이트 산탄 잡음 2q\|I<sub>g</sub>\| |

`nlev` 2 또는 3에서는 `af`가 주파수 지수이므로, kf의 단위가 nlev 0 또는 1일 때와 다릅니다.
C<sub>ox</sub>' = ε<sub>ox</sub>/tox이며, tox가 주어지지 않으면 3.453e-4 F/m²로 봅니다.

**동작점 보고:**

- MOS: 영역, id, vgs, vds, vth, gm, gds, cgs/cgd/cgb(오버랩 포함).
- BJT: ic, ib, vbe, vce, gm, β = ic/ib.
- 다이오드: vd, id, gd.
- 저항: v, i, p = v²/R.

## 표현식

`.param`, `{…}`, B 소스에 적용됩니다(`ics-sim/src/expr.rs`).

- 연산자 우선순위(낮은 것부터 높은 것 순): `?:`, `||`, `&&`, 비교(1 또는 0을 냄),
  `+ −`, `* / %`, 단항 `− + !`, 그다음 `^` 또는 `**`. 거듭제곱 연산자는 오른쪽 결합이므로
  −2^2 = −4입니다.
- 함수:
  - sqrt, exp, `log`(= ln), ln, log10, abs;
  - 삼각 함수와 쌍곡선 함수, atan2;
  - floor, ceil, int, nint/round;
  - sgn, sign, pow, pwr(x,y) = sgn(x)·|x|<sup>y</sup>;
  - min과 max(인자 개수 제한 없음), limit(x,a,b), if(c,a,b).
- **`agauss`, `gauss`, `aunif`, `unif`는 공칭값을 돌려줍니다.** 통계적 변동은 덱이 아니라
  몬테카를로에서 옵니다.

## 측정 함수

Waveform Calculator와 스펙 수식에서 쓰는 함수입니다(`ics-sim/src/calc.rs`). 복소(AC) 데이터를
측정할 때는 크기를 씁니다. 레벨 L의 상승 교차는 a < L ≤ b이고, 하강 교차는 a > L ≥ b입니다.
교차점은 선형으로 보간하며, AC 함수에서는 log f로 보간합니다.

| 함수 | 정의 |
|---|---|
| `db(w)` / `db10(w)` | 20·log10\|w\| / 10·log10\|w\| |
| `phase(w)` | 도 단위의 atan2. 연속한 점이 180°보다 크게 뛰는 곳마다 언래핑(unwrap) |
| `deriv(w)` / `integ(w)` | 중심 차분 / 누적 사다리꼴 |
| `avg(w)` / `rms(w)` | ∫y dx / 구간, √(∫y² dx / 구간), 사다리꼴 규칙으로 계산 |
| `min`, `max`, `pp`, `xmin`, `xmax` | 극값, pp = max − min, 그리고 극값이 나타나는 위치 |
| `value(w, x)` | x에서의 선형 보간 |
| `cross(w, L[, n[, edge]])` | n번째 교차의 x(n < 0이면 끝에서부터 셈) |
| `delay(w1,L1,w2,L2[,e1,e2])` | t(w2가 L2를 처음 교차) − t(w1이 L1을 처음 교차) |
| `risetime(w[,p1,p2])` / `falltime` | **파형 전체의** min..max에서 10 %와 90 %(기본값) 사이 |
| `overshoot(w)` | 100·(peak − y<sub>final</sub>)/(y<sub>final</sub> − y<sub>0</sub>) |
| `settling(w[,pct])` | w가 y<sub>final</sub> 주위로 스텝 크기의 ±pct %(기본값 2 %) 범위를 마지막으로 벗어나는 **절대 시각** |
| `bandwidth(w[,drop])` | g<sub>0</sub> − drop의 첫 하강 교차. g<sub>0</sub>는 **가장 낮은 주파수**에서의 dB 이득(피크가 아님)이고, drop의 기본값은 3.0103 dB |
| `ugf(w)` | 첫 0 dB 교차 |
| `phasemargin(w)` | ugf에서의 180 + φ(w) |
| `gainmargin(w)` | φ(w)가 처음 −180°를 지나는 곳에서의 −db(w) |

여유를 구할 때 φ는 펼친 위상에서, 가장 낮은 주파수의 위상에 가장 가까운 180°의 배수를 뺀
값입니다. 그래서 반전 입력에서 잰 응답(DC에서 약 ±180°)도 루프 이득(DC에서 0°)처럼 읽히고,
어느 입력에서 재도 여유가 같습니다.

스펙은 값이 유한하고 min ≤ v ≤ max일 때 통과합니다. 비어 있는 한계값은 한계가 없다는 뜻입니다.
플롯 아래의 측정 표에는 MIN, MAX, PEAK-PEAK, FINAL이 표시됩니다. AC에서는
GAIN @ fstart, PEAK, f −3 dB, UNITY GAIN f가 표시됩니다.

## 스펙 조언과 파생

**스펙 조언**(`ics-core/src/advice.rs`)은 수식을 보고 스펙을 분류합니다: phasemargin, gainmargin,
ugf/bandwidth, 이득(`db(`, `mag(`), 지연(`delay(`/`risetime(`/`falltime(`), 주기, 오버슈트, 정착,
전류(`i(`), 전압(`v(`). 그런 다음 벗어난 방향에 맞는 텍스트 규칙을 줍니다. 예를 들어 이득이 낮으면,
먼저 모든 증폭 소자가 포화 영역에 있는지 확인하고, 그다음 더 긴 L(더 높은 r<sub>o</sub>)이나 단마다
더 적은 전류를 씁니다. 대역폭이 낮으면 꼬리 전류를 늘리거나, 입력 소자를 넓히거나, C<sub>L</sub>을
줄입니다. 전류 미러는 I<sub>out</sub> = I<sub>ref</sub>·(W/L)<sub>out</sub>/(W/L)<sub>ref</sub>를
줍니다. 감도는 계산하지 않으며, 숫자는 최적화기와 스윕이 찾습니다.

동작점 영역:

- Level 1: V<sub>ov</sub> ≤ 0이면 차단(cutoff), V<sub>DS</sub> < V<sub>ov</sub>이면 트라이오드,
  그 밖에는 포화.
- BSIM4는 문턱 전압 이하(subthreshold)도 보고합니다.
- 포화 여유 |V<sub>DS</sub>| − (|V<sub>GS</sub>| − |V<sub>th</sub>|)가 0.1 V보다 작으면 조언이
  경고합니다.

**파생**(`ics-core/src/derive.rs`, `ics-sim/src/derive.rs`)은 조정할 변수(knob)를 고릅니다.

- 극성, 소스 넷, W, L, m이 같은 MOS 소자는 한 그룹이 됩니다. 그다음 그룹마다 역할을 줍니다.
  - **입력 쌍:** 게이트가 서로 다르고 소스가 레일이 아닌 소자 두 개;
  - **미러:** 게이트를 공유하고 그중 하나가 다이오드 연결된 소자들;
  - **전류원:** 소스가 레일에 있고 게이트를 드레인이 구동하지 않는 소자;
  - **소자:** 그 밖의 모든 것.
- 매칭된 소자는 변수 하나를 공유합니다(`M1.w,M2.w`).
- 모든 변수는 로그 스케일입니다. MOS 값은 10 nm 단위로 맞춥니다.
  - W의 범위는 [max(W<sub>0</sub>/4, W<sub>min</sub>), 4·W<sub>0</sub>]입니다.
  - L은 L<sub>0</sub> ≥ 1.2·L<sub>min</sub>일 때만 제안됩니다. 범위는
    [L<sub>0</sub>, 4·L<sub>0</sub>]이므로 줄어들지 않습니다.
  - 회로의 R과 C의 범위는 [x/4, 4x]입니다. 접지로 가는 커패시터는 조정 변수가 아니라 부하입니다.
- 전력 가드는 한계값이 [0.5·I<sub>0</sub>, 2·I<sub>0</sub>]인 `idd` 스펙을 추가합니다.

크기를 정한 뒤 파생은 결과를 확인합니다.

- 포화 영역을 벗어났거나 여유가 0.1 V 아래로 떨어진 소자를 경고합니다.
- 전원 전력이 1.5×보다 많이 늘면 경고합니다. 전력은 소스에 대해
  P = Σ −(V<sub>p</sub> − V<sub>n</sub>)·I이며, 양의 항만 셉니다.
- 줄어든 매칭 쌍을 경고합니다. 오프셋은
  k = √(W<sub>0</sub>L<sub>0</sub>/(W<sub>1</sub>L<sub>1</sub>))(Pelgrom)배로 커지며, k > 1.2이면 경고합니다.
- tphl/tplh 프리셋은 전원 소스(`vdd*` 넷의 소스) DC 값의 절반에서 측정하므로 1.8 V이면 0.9 V입니다.
  전원 소스가 없으면 0.9 V를 씁니다.

## 최적화기

최적화기는 `ics-core/src/optimize.rs`에 있고, 그 평가는
`ics-sim/src/optimize.rs`에 있습니다.

한계값이 있는 스펙만 셉니다. norm(l) = |l|(l = 0이면 1)일 때:

- **점수**(얼마나 실패했는지):
  S = Σ (min − v)/norm(min) [v < min] + (v − max)/norm(max) [v > max].
  측정이 실패하면 1e3을 더합니다.
- **여유:** M = 모든 스펙에 대한 min((v − min)/norm(min), (max − v)/norm(max))의 최솟값.

목적 함수는 최소화합니다. 실패한 점의 점수는 f = S입니다. 통과한 점의 점수는 목표에 따라
[−1, 0] 안에 있습니다.

| 목표 | 통과한 점의 목적 함수 |
|---|---|
| Margin(기본값) | −clamp(M, 0, 1) |
| Min area | −1/(1 + A/A<sub>ref</sub>), A = MOS 소자에 대한 Σ W·L·m·nf |
| Least change(파생) | −(0.5 + 0.5·clamp(M/0.05, 0, 1))/(1 + 5·Δ), Δ는 단위 좌표에서 시작점까지의 평균 거리 |

탐색 방식:

- 변수는 u ∈ [0, 1]로 매핑됩니다. 선형 변수는 x = min + u(max − min)입니다. 로그 변수는
  x = exp(ln min + u·(ln max − ln min))입니다. 그다음 x를 스텝에 맞춥니다.
- 알고리즘은 결정적 컴퍼스(패턴) 탐색입니다.
  - 회로도의 값에서 스텝 δ = 0.25로 시작합니다.
  - 각 축을 따라 ±δ를 시험하고, 처음 개선되는 쪽으로 이동합니다.
  - 개선이 없는 시험 뒤에는 δ를 절반으로 줄이고, δ < 1/1024이면 멈춥니다.
  - 예산에 도달해도 멈춥니다: 기본 60회 평가, 파생에서는 80회.
- 제약은 박스 경계와 스텝뿐입니다. 스펙 한계값은 페널티로 들어갑니다.

## 스윕, 코너, 몬테카를로, 수율

**스윕**(`ics-core/src/sweep.rs`):

- 점은 코너 × 온도 × 변수의 데카르트 곱입니다. 마지막 변수가 가장 빠르게 바뀌며, 한도는
  2000점입니다.
- `a:step:b`는 floor((b − a)/step) + 1개의 값을 줍니다.

**내장 몬테카를로**(`ics-sim/src/montecarlo.rs`). 기본값은 100회 실행, 시드 1,
vto_sigma 20 mV, kp_sigma 3 %, avt 5 mV·µm, abeta 1 %·µm입니다. 각 MOS 소자에 대해:

```
dV = dV_global(polarity) + N(0,1) · avt   / sqrt(W · Leff · m)
dB = dB_global(polarity) + N(0,1) · abeta / sqrt(W · Leff · m)
delvto = +dV (NMOS), −dV (PMOS)        mulu0 = max(1 + dB, 0.05)
level 1: vto += delvto, kp *= mulu0     BSIM4: delvto, mulu0
```

입력한 avt(또는 기본값)는 소자별 계수입니다. 쌍 데이터에서 피팅한 값
([미스매치](#매칭-쌍에서-미스매치-구하기) 참고)은 쌍 계수이며, 소자마다
√2로 나눕니다.

**스윕 매니저 몬테카를로 변수**(`McVar`):

- 값은 value = nominal + Σ coef·draw입니다. draw는 Gauss이면 N(0,1), Uniform이면 U(−1,1)입니다.
  Uniform에서 sigma는 범위의 절반입니다.
- sigma는 세 가지 방법으로 쓸 수 있습니다.
  - `5%`는 |nominal|·0.05를 뜻합니다.
  - 그냥 숫자는 그대로 씁니다.
  - `A/sqrt(wl)`은 A/√(W<sub>µm</sub>·L<sub>µm</sub>)을 뜻합니다.
- 미스매치 변수는 인스턴스마다 한 번씩 뽑습니다. 나머지는 하나의 뽑기를 공유합니다.
- 난수 생성기는 splitmix64와 Box–Muller입니다.

**실리콘에서 얻은 코너**(`ics-sim/src/corners.rs`). 코너 tt는 측정 평균입니다. 다른 코너에서는
각 주 파라미터 P를 평균 + z·σ로 옮기며, δ<sub>P</sub> = z·σ<sub>P</sub>입니다. 나머지 파라미터 a는
각각 조건부 평균을 따릅니다.

```
t_a = μ_a + Σ_aP · Σ_PP⁻¹ · δ_P
```

그다음 모델 조정 파라미터를 Levenberg–Marquardt로 피팅합니다.

**수율 예측**(`ics-core/src/predict.rs`). 각 스펙은 표준화된 뽑기 값에 대한 대리 모델(surrogate)을
갖습니다.

- 실행이 2·(1 + n + n(n+1)/2)회 이상이면 대리 모델은 2차식입니다. 그렇지 않으면 1차식이며,
  n + 3회의 실행이 필요합니다.
- 1e-6·m 릿지(ridge)를 둔 최소제곱 피팅입니다.
- 피팅 품질:
  - Q² = 1 − PRESS/SST(leave one out), PRESS = Σ(e<sub>i</sub>/(1 − h<sub>i</sub>))².
  - RMSE<sub>LOO</sub> = √(PRESS/m), 스펙의 단위.
  - Q² < 0.9이면 창이 경고합니다.
- 예측은 100 000개의 샘플을 뽑으며, 각 샘플은 y = f(x) + RMSE<sub>LOO</sub>·N(0,1)입니다.
  - 수율 = 통과 비율 × (값이 있는 실행 수 / 실행 수).
  - 결합 수율은 한계값이 있는 모든 스펙이 통과해야 합니다.
- Cpk = min((μ − LSL)/3σ, (USL − μ)/3σ).
- 입력 i의 감도는 그 주 효과 분산을 Var(y)로 나눈 값입니다. 주 효과는 Gauss 입력이면
  b² + 2c², Uniform 입력이면 b²/3 + 4c²/45입니다.
- 시뮬레이션 수율의 95 % 구간은 z = 1.96인 Wilson 구간입니다.

```
centre = (p + z²/2n) / (1 + z²/n)
half   = z · sqrt(p(1−p)/n + z²/4n²) / (1 + z²/n)
```

## 매칭 쌍에서 미스매치 구하기

`ics-core/src/mismatch.rs`(Simulation > Silicon Correlation)는 측정한 쌍
(`device, w, l, dvt[, dbeta]`)에 Pelgrom 법칙을 피팅합니다.

```
σ²(ΔX) = A² / (W·L) + σ0²          A in V·µm (Vt) or µm (β)
```

피팅은 다음과 같이 진행됩니다.

1. 행을 (W, L)로 묶습니다. 크기마다 쌍이 3개 이상, 크기가 2개 이상 있어야 합니다.
2. 각 그룹은 x = 1/(WL)에서 표본 분산 s²를 줍니다.
3. 피팅은 가중치 (n − 1)/(2·(A²x)²)(즉,
   1/Var(s²))를 쓰는 가중 최소제곱입니다.
4. σ0² < 0이면 원점을 지나는 직선으로 되돌아갑니다.

A는 **쌍 차이**의 계수입니다. 소자 하나는 σ = A/√(2WL)입니다. 시뮬레이터가 쓰는 모델은 σ0를
뺍니다.

Learn 강좌의 경험 법칙: 소자마다 σ(V<sub>th</sub>) = A<sub>Vt</sub>/√(WL)이고,
A<sub>Vt</sub> ≈ 5 mV·µm입니다. 쌍은 √2배 큽니다. 2 × 1 µm 소자는 3.5 mV입니다.

## 레이아웃 기하

여기의 값은 generic180 기준이며 단위는 nm입니다(`ics-core/src/pcell.rs`, `ics-core/src/tech.rs`).

| 레이어 | 최소 폭 | 최소 간격 |
|---|---|---|
| active | 220 | 280 |
| poly | 180 | 240 |
| metal1 | 230 | 230 |
| metal2, metal3 | 280 | 280 |
| nwell | 860 | 1400 |
| nplus, pplus | 440 | 440 |

- 콘택은 220 nm 컷, 250 nm 간격, 100/60 nm 감쌈(enclosure)입니다. via1과 via2는
  260 nm 컷, 260 nm 간격, 60 nm 감쌈입니다.
- 소자 룰: 게이트-콘택 160, 폴리 엔드캡 220, 임플란트 감쌈 180, nwell
  감쌈 430.

**MOS PCell**(w는 핑거당 폭, nf는 핑거 수):

```
plate      = max(cut + 2·enc, m1 min width)                         = 420
sd_end     = enc_bottom + cut + gate_contact                        = 480
sd_in      = cut + 2·gate_contact                                   = 540
pitch      = L + sd_in
active_len = 2·sd_end + nf·L + (nf − 1)·sd_in
w_min      = max(active min width, cut + 2·enc)                     = 420
```

- 임플란트는 액티브를 180만큼 키운 것입니다. PMOS의 nwell은 액티브를 430만큼 키운 것입니다.
- 회로도에서 핑거 수는 ceil(W/5 µm)·m이므로, 핑거 하나의 폭은 최대 5 µm입니다.
- PCell을 y 방향으로 늘이면 W가 바뀝니다. x 방향으로 늘이면 핑거마다 L이 d/nf만큼 바뀝니다.

**접합 파라미터**(AD, AS, PD, PS):

- 레이아웃이 없을 때는 값을 주지 않은 모든 MOS에 대해, 그 소자가 그려질 PCell에서 시뮬레이션 덱이
  추정합니다(PDK의 CDF 콜백과 같습니다). m의 소자 하나마다 ceil(W/5 µm)개 핑거이고, 조각은 소스,
  드레인, 소스 순으로 번갈아 놓입니다. CDL / LVS 넷리스트는 회로도를 그려진 그대로 둡니다.
- 포스트 레이아웃 넷리스트는 추정값을 추출한 값으로 바꿉니다.
- 각 확산 조각은 면적과, 게이트 에지를 뺀 둘레를 줍니다. 여러 단자가 공유하는 조각은
  똑같이 나눕니다.
- 드레인과 소스가 같은 넷에 있으면 각각 절반을 받습니다.
- generic180에서 핑거 하나는 AS = AD = 480·w, PS = PD = 960 + w입니다.
- 안쪽 조각은 A = 540·w, P = 1080입니다.
- 범용 카드는 cj = 1 mF/m², cjsw = 0.3 nF/m이므로 1 µm 핑거의 드레인은 0 V 바이어스에서 약 1 fF입니다.

추출된 소자:

- MOS: 직사각형 게이트에서는 L이 확산을 가로지르는 방향의 길이이고 W = 면적/L입니다. 꺾인
  게이트에서는 L = 면적/평균 에지, W = 평균 에지 − (1 − bend_corner)·|a − b|/2입니다.
- 저항: R = R<sub>s</sub>·L/W.
- 커패시터: C = (C<sub>area</sub>·A + C<sub>perim</sub>·P). 범용 cpip는 2.65 fF/µm²입니다.

**생성기:**

- 비아 배열: 축마다 n = floor((len − cut)/(cut + space)) + 1개의 컷을 가운데 정렬합니다.
- 가드 링: 폭 max(active, cut + 2·enc, m1) = 420, 간격 max(active space,
  2·implant enclosure) = 360.
- 트랙: 피치 = 최소 폭 + 최소 간격(metal1: 460 nm).
- 매칭 배치:
  - 단위는 폭들의 최대공약수이며, 5 µm보다 크면 절반으로 줄이고, 최대
    32 단위입니다.
  - 순서는 인터디지테이트(interdigitated) 또는 공통 중심(common centroid)입니다. 예를 들어
    [2,2]는 ABBA, [4,4]는 ABABBABA가 됩니다.
  - 8 단위부터는 두 행을 쓰며, 중심에 대해 점대칭이고, 각 행 끝에 더미를 둡니다.

**라우터**(`ics-core/src/route.rs`). 라우터는 그리드 위의 A*입니다.

- 피치는 max(pad + spacing, cut + cut space)(범용 660 nm)입니다.
- 넷은 핀 바운딩 박스의 반둘레 순서로, 짧은 것부터 배선합니다.
- 스텝 비용:
  - metal1: 어느 방향이든 4×;
  - metal2: 수평 1×, 수직 3×;
  - metal3: 수직 1×, 수평 3×.
- 비아 하나의 비용은 2 피치입니다.
- 립업(rip-up):
  - 다른 넷을 가로지르면 노드마다 20·(1 + rips)·pitch의 비용이 듭니다.
  - 한 넷은 최대 3번까지 뜯어낼 수 있습니다.

## 기생 성분 추출

`ics-core/src/pex.rs`. 도전 레이어마다 다음 계수가 있습니다.

- `area`, 단위 aF/µm²;
- `fringe`, 단위 aF/µm;
- `coupling`, 단위 aF;
- `resistance` R<sub>s</sub>, 단위 Ω/□;
- `em`, 단위 A/µm.

비아마다 컷당 저항과 컷당 EM 한계가 있습니다.

| generic180 | area | fringe | coupling | R<sub>s</sub> | em |
|---|---|---|---|---|---|
| poly | 100 | 45 | 7 | 7.5 | 0.05 mA/µm |
| metal1 | 38 | 40 | 17 | 0.08 | 1.0 mA/µm |
| metal2 | 16 | 32 | 20 | 0.08 | 1.0 mA/µm |
| metal3 | 10 | 28 | 20 | 0.06 | 1.5 mA/µm |

콘택 컷 하나는 10 Ω과 0.1 mA이고, via1 또는 via2 컷 하나는 5 Ω과 0.2 mA입니다.

**접지 커패시턴스:**

```
C_gnd = 1e-18 · Σ_layers ( area · A_µm² + fringe · P_µm )
```

A와 P는 넷의 병합된 도형에서 측정합니다. 이 항은 차폐되지 않습니다. 다른 레이어 아래에 있는
배선도 접지 커패시턴스를 그대로 유지합니다.

**측면 결합**(같은 레이어):

- C<sub>c</sub> = coupling·L/s aF이며, L은 마주 보는 길이, s는 간격입니다.
- s ≤ 5 × 최소 간격인, 서로 다른 넷의 마주 보는 평행 에지에 적용하며, 가로막히지 않은 가장 가까운
  이웃에만 적용합니다.
- 모서리 대 모서리 결합은 무시합니다.

**수직 결합:**

- 위 레이어(c<sub>hi</sub>)와 아래 레이어(c<sub>lo</sub>) 사이의 평판 계수는
  1/(1/c<sub>hi</sub> − 1/c<sub>lo</sub>) aF/µm²입니다. generic180에서는
  m1–m2 27.6, m2–m3 26.7입니다.
- 겹침 면적에서는 두 레이어 사이에 있는 레이어의 도형을 뺍니다.
- 커패시터 소자로 추출된 겹침(poly / poly2의 범용 `cpip`)은 자체 값으로 넷리스트에 들어가므로
  결합 커패시턴스로 다시 세지 않습니다.

**저항.** 배선은 최대 10 스퀘어의 타일로 나뉩니다. x 방향으로 맞닿은 두 타일은 다음 저항으로
연결됩니다.

```
R = Rs · ( w_a/(2·h_a) + w_b/(2·h_b) )
```

y 방향도 w와 h를 바꿔 같은 식을 씁니다. 비아 컷 하나는 저항 하나입니다. 1 mΩ 미만의 값은
병합합니다.

**포스트 레이아웃 넷리스트:**

- 1 aF 미만의 커패시턴스는 버립니다.
- 넷의 접지 C는 배선 면적에 비례해 RC 노드에 나눠 줍니다.
- 포트는 배선 면적이 가장 큰 노드입니다.
- MOS 단자는 게이트 중심에 가장 가까운 노드로 옮기므로, 게이트 저항이 직렬로 들어갑니다.

**넷 수치:**

- R<sub>ab</sub>는 컨덕턴스 네트워크에서 풉니다.
- "RC" 수치는 최악의 R × (C<sub>gnd</sub> + 가장 큰 결합 다섯 개)입니다. Elmore 지연이 아니라
  대략적인 상한입니다.

**배선을 그리는 동안:**

- R = R<sub>s</sub>·len/w;
- C = (area·(len + w)·w + fringe·2·(len + w))·1e-18;
- EM 한계는 I<sub>max</sub> = em·w입니다.

## IR 드롭과 일렉트로마이그레이션

`ics-core/src/emir.rs`는 같은 타일 네트워크를 씁니다.

- 브랜치 R은 최소 1 mΩ입니다.
- 패드는 넷 이름을 단 최상위 셀 라벨이며, 드롭 0으로 고정됩니다.
- 한 단자가 넷에 있는 MOS는 콘택이 닿는 곳으로 전류를 끌어갑니다. 게이트 크기 + 1 µm 안에서
  가장 가까운 metal1 타일이고, 없으면 레이어와 상관없이 게이트에 가장 가까운 타일입니다. 그래서
  게이트 위를 지나는 metal2 레일도 비아를 거쳐 전류를 받습니다. 전류는 회로도의 동작점에서
  가져오며, 레이아웃 핑거에 똑같이 나눕니다.
- 솔버는 전처리된 켤레 기울기법(preconditioned conjugate gradients)으로 G·drop = I를 상대
  잔차 1e-11까지 풉니다.
- 브랜치 전류는 I = Δdrop/R입니다.
  - 배선에서는 J = |I|/(공유 에지 µm)를 레이어의 `em`(A/µm)과 비교합니다.
  - 비아에서는 컷당 |I|를 비아의 `em`과 비교합니다.
- 해석은 DC 전용입니다. RMS나 피크 전류도, 온도 디레이팅도 없습니다. 접지 넷에서는
  드롭이 상승으로 읽힙니다.

## 공정 에뮬레이션

`ics-core/src/process.rs`와 `ics-core/src/dopants.rs`는 균일한 그리드(기본값 10 nm) 위에서 2-D
단면을 시뮬레이션합니다. 상수: k = 8.617333e-5 eV/K, q = 1.602e-19 C.

**임플란트.** 프로파일은 가우시안입니다.

```
C(d) = Q / (sqrt(2π) · ΔRp) · exp( −(d − Rp)² / (2·ΔRp²) )     cm⁻³
```

- Q는 cm⁻² 단위의 도즈이고, d는 표면 아래의 깊이입니다.
- Rp와 ΔRp는 실리콘에 대한 LSS 표에서 가져와 log-log로 보간합니다. BF₂는 E·11/49의 붕소로
  계산합니다.
- 측면 퍼짐은 σ = 0.8·ΔRp입니다.
- 모든 재료가 실리콘처럼 이온을 멈춥니다.

**확산.** D = D<sub>0</sub>·exp(−E<sub>a</sub>/kT)입니다. 솔버는 후진 오일러로, 한 번에 한 차원씩
풀며, 비반도체로는 플럭스가 들어가지 않습니다.

| 도펀트 | D<sub>0</sub> (cm²/s) | E<sub>a</sub> (eV) |
|---|---|---|
| B | 0.76 | 3.46 |
| P | 3.85 | 3.66 |
| As | 0.066 | 3.44 |
| Sb | 0.214 | 3.65 |

모델은 진성(intrinsic) 확산만 다룹니다. TED, OED, 농도 의존성, 편석(segregation)이 없습니다.

**산화.** 모델은 (100) Si에 대한 Deal-Grove입니다.

```
B   = C1 · exp(−E1/kT)  µm²/h        B/A = C2 · exp(−E2/kT)  µm/h
dry: C1 772, E1 1.23, C2 3.71e6, E2 2.00      wet: C1 386, E1 0.78, C2 9.70e7, E2 2.05
τ   = (x0² + A·x0)/B                  x(t) = ( −A + sqrt(A² + 4B·(t + τ)) ) / 2
```

- 성장한 산화막의 44 %가 실리콘을 소모합니다.
- 모델은 수직 방향만 다룹니다. 버즈 비크(bird's beak)도, 얇은 산화막 보정도 없습니다.

**판독값:**

- 이동도는 300 K에서 Caughey–Thomas를 따릅니다:
  µ = µ<sub>min</sub> + (µ<sub>max</sub> − µ<sub>min</sub>)/(1 + (N/N<sub>ref</sub>)<sup>α</sup>).
  - 전자: 68.5, 1414, 9.2e16, 0.711.
  - 정공: 44.9, 470.5, 2.23e17, 0.719.
- 면저항은 한 가지 형의 레이어에 대해 R<sub>s</sub> = 1/Σ(q·µ·|N<sub>net</sub>|·h)입니다.
- 접합은 N<sub>net</sub>의 부호가 바뀌는 곳이며, 선형 보간합니다.

**증착과 식각:**

- 컨포멀 증착이나 등방성 식각은 정확한 거리 변환을 씁니다.
- 이방성 식각은 열(column)마다 동작하며, 목록에 없는 첫 재료에서 멈춥니다.

## 픽셀과 포토다이오드

**Pinned 포토다이오드**(`ics-core/src/pinned_pd.rs`). 모델은 도핑 프로파일의 1-D 공핍
근사이며, ε<sub>Si</sub> = 11.7·ε<sub>0</sub>입니다.

- E(z) = (q/ε)∫N dz이고 φ = −∫E dz입니다.
- 공핍 경계는 전하가 균형을 이루는 곳입니다.
- 피닝 전압 V<sub>pin</sub>은 E = 0인 곳의 φ입니다.
- C/A = ε/(z* − z<sub>t</sub>) + ε/(z<sub>b</sub> − z*).
- n 도즈 ΣN·h는 담을 수 있는 전자 수의 상한입니다.

**포토다이오드 테크놀로지 파라미터**(`PhotodiodeDef`):

```
C_PD = (cap_area · A + cap_perimeter · P) · 1e-15                        F
I_dark = (dark_area · A + dark_perimeter · P) · 1e-18 · exp( (Ea/k)·(1/T_ref − 1/T) )   A
doubling temperature ΔT = k·T²·ln2 / Ea
```

E<sub>a</sub>는 공핍 영역 생성에서 0.6 eV, 확산에서 1.12 eV입니다. T<sub>ref</sub>의
기본값은 60 °C입니다. generic180에서 cap_area는 0.5 fF/µm², dark_area는 3 aA/µm²입니다.

**픽셀 테스트벤치**(Verify > Pixel Circuit, `ics-sim/src/pixel.rs`). 테스트벤치는
FD에, 그다음 PD에 500 e⁻를 주입합니다. 응답에서 다음을 구합니다.

```
C_FD = 500·q / |ΔV_FD|          CG = q / C_FD  (µV/e⁻)        A_SF = ΔV_col / ΔV_FD
C_PD = 500·q / |ΔV_PD|          full well N_FW = C_PD·(V_reset − V_sat)/q
lag  = C_PD·(V_dark − V_q)/q    (electrons left after transfer)
kTC noise = sqrt(k·T·C_FD)/q    read noise = sqrt(∫ S² df), f ≥ 1 kHz
CDS noise = sqrt(∫ S² · 4·sin²(π·f·Δt) df)       in e⁻: µV / CG_col
```

회로에서 pinned PD는 저장 커패시터와 브랜치
I = G·(min(V, V<sub>pin</sub>) − V<sub>q</sub>)로 모델링되므로, V<sub>pin</sub> 위로는 차지 않습니다.

## OPC와 더미 필

**OPC**(`ics-core/src/opc.rs`). 리소그래피 모델은 가우시안 블러 뒤에 일정한 문턱값을 적용하며,
직사각형마다 정확히 계산합니다.

```
I(x,y) = Σ ¼ [erf((x1−x)/(√2σ)) − erf((x0−x)/(√2σ))] · [erf((y1−y)/(√2σ)) − erf((y0−y)/(√2σ))]
```

- I ≥ threshold인 곳에 레지스트가 인쇄됩니다.
- EPE는 바깥쪽 법선을 따라 윤곽선까지 잰 부호 있는 거리입니다.
- 보정 스텝:
  - 첫 스텝은 −gain·EPE입니다.
  - 그 뒤 각 스텝은 기울기를 [0.3, 4]로 제한한 할선(secant) 스텝입니다.
- 기본값(w와 s는 레이어의 최소 폭과 최소 간격):

| 파라미터 | 기본값 |
|---|---|
| σ | 0.45·w |
| 문턱값(threshold) | 0.5 |
| 프래그먼트(fragment) | 2·w |
| 최대 바깥쪽 이동 | s/3 |
| 최대 안쪽 이동 | w/4 |
| 반복 횟수 | 12 |
| 이득(gain) | 0.6 |

**더미 필**(`ics-core/src/fill.rs`). 밀도 = 덮인 면적 / 윈도 면적입니다. 기본값은
다음과 같습니다.

- 정사각형 크기 max(4w, 1 µm, √min_area);
- 간격 max(2s, 0.5 µm);
- 금지 영역(keepout) max(2s, 1 µm);
- 윈도 100 µm, 스텝 50 µm(덱에 밀도 룰이 없을 때);
- 목표 min(룰의 min + 5 %, (min + max)/2).

## 공정 데이터(WAT/SPC)

`ics-core/src/process_data.rs`:

- Cp = (USL − LSL)/6σ이고 Cpk = min((μ − LSL)/3σ, (USL − μ)/3σ)이며, σ는 n − 1로 구합니다.
- 관리도는 웨이퍼 평균의 개별값 관리도(individuals chart)입니다.
  - σ̂ = 평균 이동 범위/1.128이고, 한계는 중심 ± 3σ̂입니다.
- Western Electric 규칙:
  - 1점이 3σ 밖;
  - 3점 중 2점이 2σ 밖;
  - 5점 중 4점이 1σ 밖;
  - 8점 연속으로 한쪽.
- 분산 전파:
  - 역방향: Σ<sub>k</sub> = A·Σ<sub>m</sub>·Aᵀ, A = (SᵀWS)⁻¹SᵀW,
    W = diag(1/σ²);
  - 순방향: Σ<sub>m</sub> = S·Σ<sub>k</sub>·Sᵀ.

## 강좌에서 가르치는 수식

[Learn 강좌](learn/index.md)의 손계산 식을 강좌의 숫자와 함께 정리했습니다.

| 주제 | 식 | 강좌 예 |
|---|---|---|
| 옴의 법칙, 전력 | V = IR, P = VI | 9 kΩ 양단에 1.8 V → 200 µA, 360 µW |
| RC | τ = RC, f<sub>−3dB</sub> = 1/(2πRC) | 1 kΩ, 1 nF → 1 µs, 159 kHz |
| 오버드라이브 | V<sub>ov</sub> = V<sub>GS</sub> − V<sub>th</sub> | V<sub>th</sub> = 0.45 V |
| 제곱 법칙 | I<sub>D</sub> = ½k'(W/L)V<sub>ov</sub>²(1 + λV<sub>DS</sub>) | k'<sub>n</sub> 280 µA/V², k'<sub>p</sub> 70 µA/V²; W/L 2/1, V<sub>GS</sub> 0.7 V → 17.5 µA |
| 트랜스컨덕턴스 | g<sub>m</sub> = 2I<sub>D</sub>/V<sub>ov</sub> = √(2k'(W/L)I<sub>D</sub>) | 0.2 V에서 20 µA → 200 µS |
| 출력 저항 | r<sub>o</sub> = 1/(λI<sub>D</sub>), λ = 0.08·0.5 µm/L | L = 2 µm → λ = 0.02 |
| 공통 소스 | A<sub>v</sub> = −g<sub>m</sub>(R<sub>D</sub> ‖ r<sub>o</sub>) | 145 µS, 1.43 MΩ, 50 kΩ → −7.0 |
| 대역폭 | f<sub>−3dB</sub> = 1/(2π(R<sub>D</sub>‖r<sub>o</sub>)C<sub>L</sub>), f<sub>u</sub> ≈ g<sub>m</sub>/(2πC<sub>L</sub>) | 100 fF → 33 MHz, 230 MHz |
| 미러 | I<sub>out</sub> = I<sub>ref</sub>·(W/L)<sub>2</sub>/(W/L)<sub>1</sub> | 20 µA → 20.1 µA |
| 5T OTA | A<sub>v</sub> = g<sub>m1</sub>(r<sub>o2</sub> ‖ r<sub>o4</sub>) | ≈ 121 (42 dB) |
| 인버터 스위칭 문턱 | V<sub>M</sub> = (V<sub>tn</sub> + r(V<sub>DD</sub> − \|V<sub>tp</sub>\|))/(1 + r), r = √(k'<sub>p</sub>W<sub>p</sub>/k'<sub>n</sub>W<sub>n</sub>) | W<sub>p</sub> = 2W<sub>n</sub> → 0.83 V |
| 게이트 지연 | t<sub>p</sub> ≈ 0.69·R<sub>on</sub>C<sub>L</sub> ≈ C·V<sub>DD</sub>/(2I) | 5 kΩ, 20 fF → 69 ps |
| 동적 전력 | P = α·C<sub>L</sub>·V<sub>DD</sub>²·f | 0.1·20 fF·3.24·1 GHz = 6.48 µW |
| 링 오실레이터 | T = 2·N·t<sub>p</sub> | 5 × 14 ps → 140 ps |
| Pelgrom | σ(V<sub>th</sub>) = A<sub>Vt</sub>/√(WL) | 5 mV·µm, 2 × 1 µm → 3.5 mV |
| Miller OTA | f<sub>u</sub> = g<sub>m1</sub>/(2πC<sub>c</sub>), 출력 극점 g<sub>m6</sub>/(2πC<sub>L</sub>)은 2–3× 위, R<sub>z</sub> ≈ 1/g<sub>m6</sub> | 데모 `ota_miller` |
| LDO | V<sub>out</sub> = V<sub>ref</sub>(1 + R1/R2) | 0.9 V, R1/R2 = 1/3 → 1.2 V |
| DRAM 읽기 | ΔV = (V<sub>DD</sub>/2)·C<sub>s</sub>/(C<sub>s</sub> + C<sub>BL</sub>) | 10 fF, 50 fF → ±150 mV |

## 알려진 단순화

- Level-1 MOS에는 문턱 전압 이하 전류가 없습니다. V<sub>th</sub> 아래에서는 gmin만 전류를 흘립니다.
  누설과 약반전(weak inversion)에는 BSIM4 PDK를 쓰십시오.
- Level-1 Meyer 커패시턴스(capmod 0)는 전하를 보존하지 않습니다. 전하에 민감한 회로에는
  capmod 1을 쓰십시오.
- 접지 커패시턴스는 차폐되지 않으며, 다른 레이어로의 프린지 커패시턴스는 모델링하지 않습니다.
- 배선 "RC"는 R<sub>max</sub> × C<sub>total</sub>이며, Elmore 지연이 아닙니다.
- EM은 DC 전류 밀도 검사뿐입니다.
- 레이아웃 전 접합은 PCell에서 추정합니다. 그려진 레이아웃의 확산(공유하거나 늘인 확산)이 더 클 수
  있으며, 이는 포스트 레이아웃 넷리스트에서만 보입니다.
- 공정 에뮬레이션은 진성 확산과 수직 Deal-Grove 산화를 씁니다.
- 최적화기는 국소 패턴 탐색입니다. 전역 최적점이 아니라 가까운 통과 점을 찾습니다.
