<!-- markdownlint-disable MD013 -->

# Virtuoso Layout XL과의 비교

Cadence Virtuoso Layout XL(연결성 기반 레이아웃)의 기능이 IC Studio의 어떤 기능에 대응하는지
정리했습니다(v0.2.8과 여기 적힌 변경 기준). **예**는 작업 흐름을 지원, **부분**은 더 좁은 기능이
있음, **아니요**는 없음을 뜻합니다.

## 연결성과 회로도

| Layout XL | IC Studio | 상태 |
|---|---|---|
| Generate All From Source(소자) | **Verify > Generate Layout from Schematic**: 행 배치, 확산 공유, 탭, 정합 그룹 | 예 |
| 소스에서 핀 생성 | Generate Layout과 **Update Components and Nets**가 회로도 핀마다 셀 경계에 핀 도형과 라벨을 배치(입력 왼쪽, 출력 오른쪽, 양방향 위). 플라이라인과 라우터가 연결 | 예 |
| Update Components and Nets(ECO) | 새 소자 배치, 변경 소자 제자리 재사이징, 잉여 소자 삭제(`ics eco --update`) | 예 |
| Check Against Source | 누락, 잉여, 변경 소자, 누락 핀, short, open. 편집마다 Live 재검사 | 예 |
| 플라이트 라인 / 미완성 넷 | 회로도 연결성 기반 플라이라인 | 예 |
| Mark Net, 프로빙 | **Mark Net**(`Shift+N`), 추출 기반 넷 하이라이트 | 예 |
| 회로도와 교차 선택 | 양방향 소자 크로스 프로빙 | 예 |
| 넷을 아는 Create Wire(시작 핀의 넷, 실시간 short 표시) | Path 도구가 시작한 도체의 넷을 알고, 완성된 배선이 만든 short를 알리고 표시 | 예 |
| Short locator / annotation browser | 위치가 있는 short, LVS와 DRC 마커를 Verify 패널에서 목록과 확대로 확인 | 예 |

## 배치

| Layout XL | IC Studio | 상태 |
|---|---|---|
| Placer(자동 배치) | 회로도 기반 행 배치, 컴팩션, 예제 기반 배치 | 예 |
| Modgen(정합 배열) | 차동쌍, 미러, `match`에서 더미를 포함한 공통 중심 / 인터디지트 단위 | 예 |
| Abutment | **Assist > Abut MOS Devices**와 이동 시 자동 맞붙임, 넷 확인과 자동 반전 | 예 |
| Constraint Manager | 대칭, 정렬, 고정 제약을 레이아웃에 저장, 회로도에서 추론. Constraints 목록이 위반을 표시하고 항목 선택, 되돌리기, 삭제 | 예 |
| 정렬 / 분배 | 모서리와 중심 정렬, 등간격 | 예 |

## 편집

| Layout XL / L | IC Studio | 상태 |
|---|---|---|
| 사각형, 폴리곤, 패스, 라벨, 비아, 인스턴스, 룰러 생성 | 같은 명령과 Virtuoso 바인드키 | 예 |
| 레이어 변경 시 비아를 넣는 Create Wire | Path 도구의 `+` / `-` | 예 |
| 버스 / 다중 배선 | Path 옵션의 **Wires**, **Pitch**, 배선마다 비아 | 예 |
| Width spacing pattern, 트랙 패턴 | **Shift+E > Tracks**: 균일 또는 WSP 트랙, 스냅, 적합성 검사 | 예 |
| Stretch, 부분 선택, chop, split, merge, chamfer, 폴리곤 변환 | 동일 | 예 |
| 행 / 열 복사, delta 이동 | Move / Copy 옵션의 배열 복사와 오프셋 입력 | 예 |
| Search and replace | **Edit > Search**: 종류, 레이어, 이름, 크기로 검색. 선택, 확대, 레이어 변경, 삭제 | 예 |
| PCell 스트레치 핸들, 파라미터 편집 | Stretch로 W / L / H 편집, 속성에서 모든 파라미터 편집 | 예 |
| Multi-part path, fluid guard ring | 가드 링 PCell: 직사각형(`pring` / `nring`)과 임의 맨해튼 중심선(`pring_path` / `nring_path`, 그린 패스, 폴리곤, 사각형에서 생성), 변 단위 스트레치, 생성 시 DRC 통과 | 예 |
| Edit in place, descend, 계층 트리 | 동일 | 예 |

## 설계 보조

| Layout XL / EAD | IC Studio | 상태 |
|---|---|---|
| 설계 규칙 기반 편집(DRD) | 그리는 중 DRD, 실시간 DRC, 간격 자석 | 예 |
| DRC 빠른 수정 | Quick Fix(`Ctrl+.`)와 Fix All, 수정마다 DRC로 검증 | 예(Layout XL에는 없음) |
| 전기 인지 설계: 그리는 중 R, C, EM | 배선 단위: 커서 옆 길이, R, C, EM 한계, EM 기반 폭. 넷 단위: 하이라이트한 넷의 기판 용량, 커플링, 단자 간 최악 R, R x C | 예 |
| 포스트 레이아웃 기생 성분을 시뮬레이션에 반영 | 배선 R / C, 커플링, 접합 백어노테이션 | 예 |
| IR drop / EM 분석 | Verify 패널의 **Run IR / EM** | 예 |

## 데이터와 흐름

| Virtuoso | IC Studio | 상태 |
|---|---|---|
| OpenAccess 데이터베이스 | 셀별 JSON, GDSII / OASIS / LEF / DEF. Virtuoso 라이브러리는 `cds.lib`에서 한 번에 열거나(Virtuoso가 배치 모드로 덤프) SKILL 덤프로 가져옴 | 부분(OpenAccess 자체는 링크하지 않음) |
| SKILL | CIW가 있는 레이아웃 스크립트용 SKILL 인터프리터: 언어 핵심, `db*` 도형, 인스턴스, 파라미터 인스턴스, 비아, 넷, 핀, 속성, 레이어 불리언, ROD 도형, 파일 | 레이아웃 스크립트는 예: `pcDefinePCell`, ROD 멀티파트 패스, SKILL++ 클로저, 대화 상자로 표시되는 `hi*` 폼 |
| Calibre / PVS 사인오프 | Calibre, KLayout, netgen 실행과 캔버스 결과 표시, SVRF를 내장 DRC로 변환 | 예 |

## 아직 없는 것

- Virtuoso 없이 OpenAccess 파일 읽기: Si2 OpenAccess 라이브러리가 필요하며 링크하지 않습니다. Virtuoso가
  설치되어 있으면 `cds.lib`에서 한 번에 열리고, 없으면 다른 곳에서 만든 SKILL 덤프나 GDSII / OASIS로
  가져옵니다.
