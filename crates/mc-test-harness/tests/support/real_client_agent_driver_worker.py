import contextlib
import importlib.util
import io
import json
import sys
import traceback


driver_path = sys.argv[1]
spec = importlib.util.spec_from_file_location("solaris_real_client_agent_driver", driver_path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

for line in sys.stdin:
    request = json.loads(line)
    captured_stdout = io.StringIO()
    captured_stderr = io.StringIO()
    previous_argv = sys.argv
    try:
        sys.argv = [driver_path, *request["args"]]
        with (
            contextlib.redirect_stdout(captured_stdout),
            contextlib.redirect_stderr(captured_stderr),
        ):
            code = module.main()
    except SystemExit as exc:
        code = int(exc.code or 0)
    except BaseException:
        code = 1
        traceback.print_exc(file=captured_stderr)
    finally:
        sys.argv = previous_argv
    print(
        json.dumps(
            {
                "code": code,
                "stdout": captured_stdout.getvalue(),
                "stderr": captured_stderr.getvalue(),
            }
        ),
        flush=True,
    )
