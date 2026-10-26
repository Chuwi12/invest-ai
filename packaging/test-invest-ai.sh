#!/usr/bin/env bash
# Pruebas del script ./invest-ai y de las unidades de packaging/, sin tocar el servicio real:
# proyecto temporal, XDG_* temporales, `systemctl` y `ollama` simulados en el PATH y un Ollama falso
# (HTTP) en un puerto libre. Uso: packaging/test-invest-ai.sh   (sale con 1 si algo falla)
set -Eeuo pipefail

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="$(dirname -- "$HERE")"
TMP="$(mktemp -d)"
FAILS=0
SERVER_PID=""
SVC_PID="" # «proceso principal del servicio» simulado

cleanup() {
  [[ -z $SERVER_PID ]] || kill "$SERVER_PID" 2>/dev/null || true
  [[ -z $SVC_PID ]] || kill "$SVC_PID" 2>/dev/null || true
  chmod -R u+w -- "$TMP" 2>/dev/null || true # las versiones instaladas son de solo lectura
  rm -rf -- "${TMP:?}"
}
trap cleanup EXIT

pass() { printf '  ok    %s\n' "$*"; }
fail() { printf '  FALLO %s\n' "$*" >&2; FAILS=$((FAILS + 1)); }
check() { # check DESCRIPCIÓN COMANDO...
  local desc=$1
  shift
  if "$@" >/dev/null 2>&1; then pass "$desc"; else fail "$desc"; fi
}
contains() { grep -qF -- "$2" <<<"$1"; }
lacks() { ! contains "$@"; }
not_writable() { [[ ! -w $1 ]]; }
NL=$'\n' # nombres de directorio con salto de línea
aborts() { # aborts DESCRIPCIÓN CÓDIGO SALIDA TEXTO : salió con error y la salida dice TEXTO
  if (( $2 != 0 )) && contains "$3" "$4"; then pass "$1"; else fail "$1"; fi
}

# ---------- entorno simulado ----------
PROJ="$TMP/proj"
BINS="$TMP/bin"
XDG="$TMP/xdg"
mkdir -p "$PROJ/frontend/dist/assets" "$PROJ/target/release" "$PROJ/data" "$BINS" "$XDG"/{cfg,data,state}
cp "$REPO/invest-ai" "$REPO/.env.example" "$PROJ/"
cp -r "$REPO/packaging" "$PROJ/packaging"
printf '<html>v1</html>\n' >"$PROJ/frontend/dist/index.html"
printf 'console.log(1)\n' >"$PROJ/frontend/dist/assets/a.js"
printf '#!/usr/bin/env bash\necho binario-falso\n' >"$PROJ/target/release/invest-ai"
chmod +x "$PROJ/target/release/invest-ai"

cat >"$BINS/systemctl" <<EOF
#!/usr/bin/env bash
echo "systemctl \$*" >>"$TMP/systemctl.log"
case "\$*" in
  *is-active*) exit 3 ;;
  *is-enabled*) exit 1 ;;
  *"show -p MainPID"*) cat "$TMP/mainpid" 2>/dev/null || echo 0; exit 0 ;;
esac
exit 0
EOF
cat >"$BINS/ollama" <<EOF
#!/usr/bin/env bash
echo "OLLAMA_HOST=\${OLLAMA_HOST-} \$*" >>"$TMP/ollama.log"
EOF
chmod +x "$BINS/systemctl" "$BINS/ollama"
: >"$TMP/ollama.log"

PORT="$(python3 -I -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
DEAD="$(python3 -I -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
cat >"$TMP/fake_ollama.py" <<'EOF'
import http.server, json, sys
port, models = int(sys.argv[1]), [m for m in sys.argv[2].split(",") if m]
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        body = {"version": "0.0-test"} if self.path == "/api/version" else {"models": [{"name": m} for m in models]} if self.path == "/api/tags" else None
        if body is None:
            self.send_response(404); self.end_headers(); return
        raw = json.dumps(body, separators=(",", ":")).encode()
        self.send_response(200); self.send_header("content-type", "application/json"); self.end_headers(); self.wfile.write(raw)
http.server.HTTPServer(("127.0.0.1", port), H).serve_forever()
EOF
python3 -I "$TMP/fake_ollama.py" "$PORT" "otro:1b" >/dev/null 2>&1 &
SERVER_PID=$!
for _ in $(seq 1 50); do curl -fsS --max-time 1 -o /dev/null "http://127.0.0.1:$PORT/api/version" 2>/dev/null && break; sleep 0.1; done

# run_script ARGS... : el script de la copia temporal, con todo aislado
run_script() {
  PATH="$BINS:$PATH" XDG_CONFIG_HOME="$XDG/cfg" XDG_DATA_HOME="$XDG/data" XDG_STATE_HOME="$XDG/state" \
    CARGO_TARGET_DIR="$PROJ/target" BIND_ADDR="${BIND_ADDR:-127.0.0.1:1}" "$PROJ/invest-ai" "$@"
}
ollama_calls() { wc -l <"$TMP/ollama.log"; }

# ---------- _prestart y pull ----------
echo "== _prestart: avisa y sigue; nunca descarga sin consentimiento =="
out=$(LLM_BASE_URL="http://127.0.0.1:$PORT" LLM_MODEL=qwen2.5:7b run_script _prestart 2>&1) && rc=0 || rc=$?
check "modelo ausente: sale con 0 (la app arranca)" test "$rc" -eq 0
check "modelo ausente: avisa en el log" contains "$out" "no está instalado"
check "modelo ausente: NO llama a ollama pull" test "$(ollama_calls)" -eq 0
out=$(AUTO_PULL_MODEL=1 LLM_BASE_URL="http://127.0.0.1:$PORT" LLM_MODEL=qwen2.5:7b run_script _prestart 2>&1) && rc=0 || rc=$?
check "AUTO_PULL_MODEL=1: descarga (con -- delante del nombre)" grep -q 'pull -- qwen2.5:7b' "$TMP/ollama.log"
out=$(AUTO_PULL_MODEL=0 LLM_BASE_URL="http://127.0.0.1:$PORT" LLM_MODEL=zzz:1b run_script _prestart 2>&1) || true
check "AUTO_PULL_MODEL=0 no descarga" test "$(ollama_calls)" -eq 1
out=$(LLM_BASE_URL="http://127.0.0.1:$PORT" LLM_MODEL=otro:1b run_script _prestart 2>&1) && rc=0 || rc=$?
check "modelo presente: ok" contains "$out" "disponible"
start=$SECONDS
out=$(INVEST_AI_PRESTART_WAIT=2 LLM_BASE_URL="http://127.0.0.1:$DEAD" run_script _prestart 2>&1) && rc=0 || rc=$?
check "sin Ollama: espera, avisa y sale con 0 (no bloquea el arranque)" test "$rc" -eq 0
check "sin Ollama: el aviso lo dice" contains "$out" "arranco la app igualmente"
check "sin Ollama: no espera más de lo configurado" test $((SECONDS - start)) -lt 15
out=$(BIND_ADDR=0.0.0.0:8787 LLM_BASE_URL="http://127.0.0.1:$PORT" run_script _prestart 2>&1) && rc=0 || rc=$?
check "BIND_ADDR no loopback: _prestart SÍ falla" test "$rc" -ne 0

echo "== pull =="
: >"$TMP/ollama.log"
out=$(LLM_BASE_URL="http://127.0.0.1:$PORT" LLM_MODEL=qwen2.5:7b run_script pull 2>&1) && rc=0 || rc=$?
check "pull sin argumento descarga LLM_MODEL (con -- delante del nombre)" grep -q 'pull -- qwen2.5:7b' "$TMP/ollama.log"
out=$(LLM_BASE_URL="http://127.0.0.1:$PORT" run_script pull otro:1b 2>&1) || true
check "pull de un modelo ya instalado no descarga" test "$(ollama_calls)" -eq 1
out=$(LLM_BASE_URL="http://127.0.0.1:$PORT" run_script pull -- --x 2>&1) && rc=0 || rc=$?
check "pull rechaza nombres que empiezan por -" test "$rc" -ne 0
out=$(LLM_BASE_URL="http://127.0.0.1:$DEAD" run_script pull qwen2.5:7b 2>&1) && rc=0 || rc=$?
check "pull sin Ollama falla con un mensaje" contains "$out" "Ollama no responde"

echo "== LLM_MODEL no válido: no llega a ollama (B-3) =="
: >"$TMP/ollama.log"
UTF8="$(locale -a | grep -im1 '^es_ES\.utf-\?8$' || true)" # con es_ES.UTF-8 [A-Za-z] aceptaría «á»
for m in --insecure -x 'qwen2.5:7b --insecure' 'modeláb:1b' 'a b'; do
  [[ $m != *á* ]] || [[ -n $UTF8 ]] || continue
  out=$(AUTO_PULL_MODEL=1 LC_ALL="${UTF8:-C}" LLM_BASE_URL="http://127.0.0.1:$PORT" LLM_MODEL="$m" run_script _prestart 2>&1) && rc=0 || rc=$?
  check "_prestart con AUTO_PULL_MODEL=1 y LLM_MODEL='$m': sale con 0" test "$rc" -eq 0
  check "...avisa de que el nombre no es válido" contains "$out" "nombre de modelo no válido"
  check "...y NO llega a ollama" test "$(ollama_calls)" -eq 0
done
out=$(LC_ALL="${UTF8:-C}" LLM_BASE_URL="http://127.0.0.1:$PORT" run_script pull 'modeláb:1b' 2>&1) && rc=0 || rc=$?
aborts "pull con un nombre con tilde falla y NO llega a ollama" "$rc" "$out" "nombre de modelo no válido"
check "...ollama sigue sin llamadas" test "$(ollama_calls)" -eq 0

# ---------- install: versión instalada inmutable ----------
echo "== install: copia inmutable =="
run_script install --no-build >"$TMP/install1.log" 2>&1 || { cat "$TMP/install1.log"; fail "install --no-build"; }
UNIT="$XDG/cfg/systemd/user/invest-ai.service"
OLLAMA_UNIT="$XDG/cfg/systemd/user/invest-ai-ollama.service"
REL="$(sed -n 's/^ExecStart=//p' "$UNIT" | head -n 1)"
REL="${REL%/bin/invest-ai}"
check "la unidad apunta a releases/<id>" test "${REL#"$XDG/data/invest-ai/releases/"}" != "$REL"
check "binario, script, frontend y VERSION copiados" test -x "$REL/bin/invest-ai" -a -x "$REL/invest-ai" -a -f "$REL/frontend/dist/index.html" -a -f "$REL/VERSION"
check "ExecStartPre usa el script de la copia" grep -qx "ExecStartPre=$REL/invest-ai _prestart" "$UNIT"
check "STATIC_DIR apunta al frontend de la copia" grep -qx "Environment=STATIC_DIR=$REL/frontend/dist" "$UNIT"
check "INVEST_AI_ROOT y WorkingDirectory siguen en el proyecto" grep -qx "Environment=INVEST_AI_ROOT=$PROJ" "$UNIT"
check "la copia es de solo lectura" bash -c "! echo x >>'$REL/frontend/dist/index.html'"
if command -v systemd-analyze >/dev/null 2>&1; then
  check "systemd-analyze --user verify de las dos unidades" bash -c "cd /tmp && systemd-analyze --user verify '$UNIT' '$OLLAMA_UNIT'"
fi
check "el respaldo lleva sus dos ExecCondition" test "$(grep -c '^ExecCondition=' "$OLLAMA_UNIT")" -eq 2
check "una de ellas consulta ollama.service del sistema" grep -q 'systemctl --system is-enabled --quiet ollama.service' "$OLLAMA_UNIT"

echo "== la copia no depende del árbol de trabajo =="
printf '#!/usr/bin/env bash\necho ARBOL-EJECUTADO >&2\nexit 99\n' >"$PROJ/invest-ai"
rm -f "$PROJ/target/release/invest-ai"
printf '<html>ENVENENADO</html>\n' >"$PROJ/frontend/dist/index.html"
out=$(INVEST_AI_ROOT="$PROJ" LLM_BASE_URL="http://127.0.0.1:$PORT" LLM_MODEL=otro:1b PATH="$BINS:$PATH" "$REL/invest-ai" _prestart 2>&1) && rc=0 || rc=$?
check "_prestart de la copia funciona con el script del repo roto" test "$rc" -eq 0
check "...y no ejecutó el del repo" bash -c "! grep -q ARBOL <<<'$out'"
check "la copia conserva el frontend original" grep -q 'v1' "$REL/frontend/dist/index.html"
cp "$REPO/invest-ai" "$PROJ/invest-ai"
printf '#!/usr/bin/env bash\necho binario-falso\n' >"$PROJ/target/release/invest-ai"
chmod +x "$PROJ/target/release/invest-ai"

echo "== install: idempotente, actualiza y poda =="
printf '<html>v1</html>\n' >"$PROJ/frontend/dist/index.html"
n_before=$(find "$XDG/data/invest-ai/releases" -mindepth 1 -maxdepth 1 -type d | wc -l)
run_script install --no-build >"$TMP/install2.log" 2>&1 || fail "segunda instalación"
check "reinstalar lo mismo no crea otra versión" test "$(find "$XDG/data/invest-ai/releases" -mindepth 1 -maxdepth 1 -type d | wc -l)" -eq "$n_before"
for i in 2 3 4 5; do
  printf '<html>v%s</html>\n' "$i" >"$PROJ/frontend/dist/index.html"
  sleep 1.1 # mtime distinto entre versiones: la poda ordena por fecha
  run_script install --no-build >"$TMP/install-v$i.log" 2>&1 || fail "instalación v$i"
done
check "tras cambiar el contenido hay otra versión y la poda deja 3" test "$(find "$XDG/data/invest-ai/releases" -mindepth 1 -maxdepth 1 -type d | wc -l)" -eq 3
NEW="$(sed -n 's/^ExecStart=//p' "$UNIT" | head -n 1)"
check "la unidad apunta a la última" test -x "$NEW"
rm -f "$PROJ/target/release/invest-ai"
out=$(run_script install --no-build 2>&1) && rc=0 || rc=$?
check "install --no-build sin compilación falla con un mensaje" bash -c "test $rc -ne 0 && grep -qF 'falta la compilación' <<<'$out'"
printf '#!/usr/bin/env bash\necho binario-falso\n' >"$PROJ/target/release/invest-ai"
chmod +x "$PROJ/target/release/invest-ai"

# Cambia el contenido de dist/ (otra versión) e instala. Las pausas dan mtime distinto a las carpetas: la poda ordena por fecha.
install_new() { # install_new ETIQUETA
  printf '<html>%s</html>\n' "$1" >"$PROJ/frontend/dist/index.html"
  sleep 0.2
  run_script install --no-build >"$TMP/install-$1.log" 2>&1 || { cat "$TMP/install-$1.log"; fail "instalación $1"; }
}
RELS="$XDG/data/invest-ai/releases"
unit_rel() { sed -n 's/^ExecStart=//p' "$UNIT" | head -n 1 | sed 's|/bin/invest-ai$||'; }
n_versions() { find "$RELS" -mindepth 1 -maxdepth 1 -type d -regextype posix-extended -regex '.*/[0-9a-f]{12}' | wc -l; }
n_tmp() { find "$RELS" -mindepth 1 -maxdepth 1 -name '.tmp-*' | wc -l; }

echo "== poda: nombres con saltos de línea, '..' y enlaces (B-1) =="
VICTIM="$TMP/victima" # fuera de releases/: la poda no puede cambiarle los permisos ni borrar nada de ahí
mkdir -p "$VICTIM/sub"
: >"$VICTIM/f"
chmod 400 "$VICTIM/f"
chmod 500 "$VICTIM/sub" "$VICTIM"
ln -s "$VICTIM" "$RELS/a" # con el nombre «a\nb» la ruta «releases/a» se colaba en la lista de la poda
mkdir "$RELS/a${NL}b" "$RELS/..${NL}x" "$RELS/viejo-manual"
ln -s "$VICTIM" "$RELS/0123456789ab" # enlace con nombre de versión válido
touch -d '2020-01-01' "$RELS/a${NL}b" "$RELS/..${NL}x" "$RELS/viejo-manual"
UNIT_REL_BEFORE="$(unit_rel)"
install_new v6
check "la víctima fuera de releases/ conserva sus permisos (dir 500, fichero 400, subdir 500)" test "$(stat -c %a "$VICTIM" "$VICTIM/f" "$VICTIM/sub" | tr '\n' ' ')" = "500 400 500 "
check "ni se borró nada de ella" test -f "$VICTIM/f" -a -d "$VICTIM/sub"
check "la versión nueva sigue de solo lectura (el '..' no cambió permisos del árbol de releases)" not_writable "$(unit_rel)/VERSION"
check "...y la anterior también" not_writable "$UNIT_REL_BEFORE/VERSION"
check "los nombres hostiles y los enlaces siguen donde estaban" test -d "$RELS/a${NL}b" -a -d "$RELS/..${NL}x" -a -L "$RELS/a" -a -L "$RELS/0123456789ab"
check "un directorio que no es una versión no se borra" test -d "$RELS/viejo-manual"
check "las versiones legítimas se podan como siempre (quedan 3)" test "$(n_versions)" -eq 3

echo "== poda: conserva la versión que usa el servicio en marcha (#6) =="
RUN="$(unit_rel)"
chmod u+w "$RUN/bin" "$RUN/bin/invest-ai" # el «binario» corre desde la copia
cp "$(command -v sleep)" "$RUN/bin/invest-ai" || fail "no se pudo simular el proceso del servicio"
chmod a-w "$RUN/bin" "$RUN/bin/invest-ai"
"$RUN/bin/invest-ai" 600 &
SVC_PID=$!
echo "$SVC_PID" >"$TMP/mainpid"
for v in v7 v8 v9 v10; do install_new $v; done
check "4 instalaciones sin reiniciar: la versión en marcha sigue en releases/ con su frontend" test -f "$RUN/frontend/dist/index.html"
check "se conservan las 3 recientes y la de en marcha" test "$(n_versions)" -eq 4
kill "$SVC_PID" 2>/dev/null || true
wait "$SVC_PID" 2>/dev/null || true
SVC_PID=""
echo 0 >"$TMP/mainpid"
install_new v11
check "al parar el servicio, la versión ya no se conserva" test ! -e "$RUN"
check "...y quedan las 3 recientes" test "$(n_versions)" -eq 3

echo "== install: no deja temporales (#7) =="
FAILBIN="$TMP/failbin"
mkdir -p "$FAILBIN"
cat >"$FAILBIN/install" <<EOF
#!/usr/bin/env bash
for a in "\$@"; do [[ \$a == */.tmp-*/* ]] && exit 1; done
exec $(command -v install) "\$@"
EOF
chmod +x "$FAILBIN/install"
printf '<html>v12</html>\n' >"$PROJ/frontend/dist/index.html"
PATH="$FAILBIN:$PATH" run_script install --no-build >"$TMP/install-falla.log" 2>&1 && rc=0 || rc=$?
check "una copia que falla a mitad hace fallar install" test "$rc" -ne 0
check "...y no deja .tmp-* en releases/" test "$(n_tmp)" -eq 0
check "...ni una versión a medias" test "$(n_versions)" -eq 3
mkdir -p "$RELS/.tmp-OLD123/x"
: >"$RELS/.tmp-OLD123/x/f"
chmod 400 "$RELS/.tmp-OLD123/x/f"
chmod 500 "$RELS/.tmp-OLD123/x"
mkdir "$RELS/.tmp-NEW456" "$RELS/.tmp-ab${NL}cd"
ln -s "$VICTIM" "$RELS/.tmp-LINK12"
touch -d '3 hours ago' "$RELS/.tmp-OLD123" "$RELS/.tmp-ab${NL}cd"
run_script install --no-build >"$TMP/install-tmp.log" 2>&1 || fail "install con temporales viejos"
check "la poda borra el temporal abandonado (más de 1 h), aunque tenga contenido de solo lectura" test ! -e "$RELS/.tmp-OLD123"
check "...pero no el reciente (otro install puede estar en marcha)" test -d "$RELS/.tmp-NEW456"
check "...ni el de nombre hostil ni el enlace" test -d "$RELS/.tmp-ab${NL}cd" -a -L "$RELS/.tmp-LINK12"
check "...y la víctima sigue intacta" test "$(stat -c %a "$VICTIM" "$VICTIM/f" "$VICTIM/sub" | tr '\n' ' ')" = "500 400 500 "
rmdir "$RELS/.tmp-NEW456"

echo "== install: DATABASE_URL que abriría ReadWritePaths de más (B-2) =="
mkdir -p "$TMP/home"
MODES_BEFORE="$(stat -c %a "$TMP/home" "$PROJ" "$XDG/data/invest-ai" | tr '\n' ' ')"
for d in "$XDG/data/invest-ai" "$RELS" "$RELS/datos" "$XDG/data" "$XDG/cfg/systemd/user" "$XDG/cfg" "$PROJ" "$TMP" "$TMP/home" /; do
  out=$(HOME="$TMP/home" DATABASE_URL="sqlite://${d%/}/invest-ai.db" run_script install --no-build 2>&1) && rc=0 || rc=$?
  aborts "DATA_DIR=$d: install aborta y lo explica" "$rc" "$out" "los datos"
done
check "...sin crear ni cambiar permisos (ni \$HOME, ni el proyecto, ni invest-ai/)" test "$(stat -c %a "$TMP/home" "$PROJ" "$XDG/data/invest-ai" | tr '\n' ' ')" = "$MODES_BEFORE" -a ! -e "$RELS/datos"
OTHER="$XDG/data/invest-ai-datos" # comparte prefijo de texto con invest-ai/, pero no es ni lo contiene
out=$(DATABASE_URL="sqlite://$OTHER/invest-ai.db" run_script install --no-build 2>&1) && rc=0 || rc=$?
check "una carpeta de datos propia (hermana, con prefijo parecido) sí se instala" test "$rc" -eq 0
check "...y la unidad la usa en ReadWritePaths" grep -qx "ReadWritePaths=$OTHER" "$UNIT"
run_script install --no-build >"$TMP/install-default.log" 2>&1 || fail "reinstalar con los datos por defecto"
check "con los datos por defecto ReadWritePaths vuelve a ser data/ del proyecto" grep -qx "ReadWritePaths=$PROJ/data" "$UNIT"

echo "== status: versión instalada ausente (#8) =="
STATUS() { LLM_BASE_URL="http://127.0.0.1:$DEAD" run_script status 2>&1; }
GONE="$(unit_rel)"
GONE_ID="${GONE##*/}"
out=$(STATUS)
check "status con la versión presente la muestra" contains "$out" "versión instalada: $GONE_ID"
chmod -R u+w "$GONE"
rm -rf "$GONE"
out=$(STATUS)
check "versión instalada borrada: status dice cuál falta" contains "$out" "la versión instalada $GONE_ID no existe"
check "...y propone repetir install" contains "$out" "repite ./invest-ai install"
check "...sin llamarla instalación antigua" lacks "$out" "instalación antigua"
run_script install --no-build >"$TMP/install-repair.log" 2>&1 || fail "repetir install"
out=$(STATUS)
check "tras repetir install, status vuelve a mostrarla" contains "$out" "versión instalada: $GONE_ID"
cp "$UNIT" "$TMP/unit.bak"
sed -i "s|^ExecStart=.*|ExecStart=$PROJ/target/release/invest-ai|" "$UNIT"
out=$(STATUS)
check "unidad que apunta al árbol de trabajo: sigue diciendo instalación antigua" contains "$out" "instalación antigua"
cp "$TMP/unit.bak" "$UNIT"

echo "== uninstall =="
run_script uninstall >"$TMP/uninstall.log" 2>&1 || fail "uninstall"
check "uninstall retira las unidades y las versiones" bash -c "! test -e '$UNIT' && ! test -e '$XDG/data/invest-ai/releases'"
check "uninstall no toca .env ni data/" test -f "$PROJ/.env" -a -d "$PROJ/data"

echo
if (( FAILS )); then
  printf '%s prueba(s) fallida(s)\n' "$FAILS" >&2
  exit 1
fi
echo "todo OK"
