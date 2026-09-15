"""Read a validation asset through GitHub; credentials never follow redirects."""
import os
from pathlib import Path
import re
import sys
from urllib.error import HTTPError
from urllib.parse import urlsplit
from urllib.request import build_opener, HTTPRedirectHandler, Request

API = 'https://api.github.com/repos/tampopogk/kanna/releases/assets/'
ASSET_HOSTS = {'release-assets.githubusercontent.com', 'objects.githubusercontent.com'}
MAX_BYTES = 256 * 1024 * 1024


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def fetch(asset_id: str, token: str, destination: Path, opener=None) -> None:
    if not re.fullmatch(r'[1-9][0-9]*', asset_id) or not token:
        raise ValueError('a numeric GitHub asset ID and job token are required')
    opener = opener or build_opener(NoRedirect())
    request = Request(API + asset_id, headers={
        'Authorization': 'Bearer ' + token,
        'Accept': 'application/octet-stream',
        'X-GitHub-Api-Version': '2022-11-28',
    })
    try:
        response = opener.open(request, timeout=60)
    except HTTPError as error:
        if error.code != 302:
            # Do not echo response bodies, tokens or signed redirect URLs.
            raise RuntimeError(f'GitHub asset API returned HTTP {error.code}; draft assets may require permissions unavailable to contents:read. Keep the release draft.') from None
        location = error.headers.get('Location', '')
        target = urlsplit(location)
        if (target.scheme != 'https' or target.hostname not in ASSET_HOSTS
                or target.username or target.password or target.port not in (None, 443)):
            raise RuntimeError('GitHub asset redirected outside the allowed HTTPS asset hosts') from None
        # A separate request with NO Authorization. No automatic redirects,
        # including a second redirect by storage back to an arbitrary host.
        try:
            response = opener.open(Request(location), timeout=60)
        except HTTPError as redirected_error:
            raise RuntimeError(f'GitHub asset storage returned HTTP {redirected_error.code}; no redirect followed') from None
    with response, destination.open('xb') as output:
        received = 0
        while data := response.read(1024 * 1024):
            received += len(data)
            if received > MAX_BYTES:
                raise RuntimeError('validation bundle exceeds the bounded asset size')
            output.write(data)


if __name__ == '__main__':
    try:
        fetch(os.environ.get('PAIR_ASSET_ID', ''), os.environ.get('GH_TOKEN', ''), Path(sys.argv[1]))
    except Exception as error:
        # The urllib network exception can include a signed URL; keep unexpected
        # transport diagnostics to the type. Our own diagnostics contain none.
        print(str(error) if isinstance(error, (RuntimeError, ValueError)) else type(error).__name__, file=sys.stderr)
        sys.exit(1)
