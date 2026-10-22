# 설정 JSON 이동

2026-10-23. 연결 프로필과 앱 환경설정을 단일 JSON으로 내보내고 새 설정 폴더에 복원한다.
DB 데이터 백업이나 테이블 schema migration 기능과는 별개다.

## GUI

1. `⌘,` → **설정 이동**을 연다.
2. **기준 폴더**를 선택하고 **JSON 내보내기**로 새 파일에 저장한다.
   이 버튼은 저장된 최신 연결·환경설정을 읽는다. 기준 폴더 내부 SQLite/CA 경로는 상대경로가 된다.
3. 다른 환경에서 **JSON 가져오기**로 파일을 선택한다. 프로필 수·설정·경로 정보를 검토한다.
4. 상대 파일 경로의 **새 기준 폴더**를 선택한다. 기본값은 선택한 JSON의 부모 폴더이며 화면에 표시된다.
5. 존재하지 않는 **새 설정 폴더** 이름을 지정하고 **복원 후 새 창 열기**를 누른다.
   기존 창과 SQL 초안은 유지된다. 새 창은 자동으로 DB에 연결하지 않는다.

복원된 폴더는 **설정 폴더 열기**로 다시 열 수 있다. 같은 설정 디렉터리가 이미 열려 있으면
기존 창을 활성화한다. 복원은 기본 설정 위치를 바꾸지 않는다. 다음 실행의 설정을 지정하려면
`DBIDE_PROFILE_STORE=/absolute/config/profiles.json`으로 GUI를 실행하거나 CLI `--config`를 사용한다.

## 포함 범위

- 연결 이름·엔진·호스트·포트·사용자명·DB 이름·TLS 옵션·SQLite/CA 경로
- 선택한 프로필 ID
- 한·영 언어, 글꼴·탭 너비, 테이블 페이지 크기, 패널 크기
- 편집 내용 복구의 활성화 여부

비밀번호·Keychain 내용·SQL 실행 기록·미저장 SQL·프로젝트 경로·결과·metadata 캐시는 포함하지 않는다.
SQLite 파일·CA 파일·워크스페이스 SQL 파일은 별도로 옮겨야 한다.
프로필 ID를 보존하므로 같은 컴퓨터에서 사용자가 명시적으로 Keychain 불러오기를 누르면 기존 ID의
비밀번호와 매칭될 수 있다. 가져오기 자체는 Keychain을 읽지 않는다.

## 경로와 버전

내보내기 기준 폴더 밖의 경로는 OS가 표시된 절대경로로 보존한다. 동일 OS에서도 파일 위치를 확인해야 한다.
다른 OS의 절대경로는 추측해서 바꾸지 않는다. CLI `--map`으로 원래 경로와 새 경로를 정확히 지정한다.
상대 경로를 사용하는 원본 프로필은 해당 경로를 해석할 원래 기준 폴더를 명시해야 한다.
`..`, drive-relative 경로, 지원하지 않는 Unix 역슬래시 파일명은 거부한다.

Bundle schema v1 / profiles v2 / settings v1. 2MiB 입력 제한, 알 수 없는 필드·미래 버전·잘못된
프로필·설정은 거부한다. 기존 profile v1은 ProfileStore가 읽으며 v2로 내보낸다.

복원은 새 디렉터리를 독점 생성하고 `profiles.json`, `app-settings.json`,
`editor-preferences.json`, 마지막에 `portable-complete.json`을 기록한다. 기존 폴더는 빈 폴더여도
덮어쓰지 않는다. Unix에서 폴더 0700, 파일 0600을 사용한다. 여러 파일을 하나의 filesystem
transaction으로 저장하는 기능은 아니다. 정상 오류는 생성한 파일을 정리하며, 프로세스 강제 종료나
전원 장애 뒤에는 완료 marker가 없는 불완전한 새 폴더가 남을 수 있다.

## CLI

```sh
dbide --config /source/config/profiles.json config export \
  --base-dir /source/project --output /transfer/dbide-settings.json
dbide config inspect --file /transfer/dbide-settings.json
dbide config import --file /transfer/dbide-settings.json \
  --path-root /destination/project --destination /destination/new-config \
  --map '/old/cert/ca.pem=/destination/cert/ca.pem'
```

`--profiles`는 `--config`의 alias다. inspect/import는 현재 활성 설정을 수정하지 않는다.
JSON report에 복원된 프로필 경로와 누락 파일 등의 경고가 나온다. [전체 CLI 안내](CLI.md).
