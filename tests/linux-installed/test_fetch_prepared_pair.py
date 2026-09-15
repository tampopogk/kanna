import importlib.util
import io
from pathlib import Path
import tempfile
import unittest
from urllib.error import HTTPError

spec = importlib.util.spec_from_file_location('fetch_pair', Path(__file__).with_name('fetch-prepared-pair.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class Opener:
    def __init__(self, location=None, status=302):
        self.requests = []
        self.location = location
        self.status = status

    def open(self, request, timeout):
        self.requests.append(request)
        if len(self.requests) == 1 and self.location is not None:
            raise HTTPError(request.full_url, self.status, 'test', {'Location': self.location}, None)
        return io.BytesIO(b'exact validation bytes')


class FetchTests(unittest.TestCase):
    def test_direct_api_response(self):
        opener = Opener()
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'pair.tar'
            module.fetch('123', 'synthetic-test-token', path, opener)
            self.assertEqual(path.read_bytes(), b'exact validation bytes')
        self.assertEqual(opener.requests[0].full_url, module.API + '123')
        self.assertEqual(opener.requests[0].get_header('Authorization'), 'Bearer synthetic-test-token')

    def test_github_storage_redirect_drops_authorization(self):
        opener = Opener('https://release-assets.githubusercontent.com/path?signed=test')
        with tempfile.TemporaryDirectory() as directory:
            module.fetch('123', 'synthetic-test-token', Path(directory) / 'pair.tar', opener)
        self.assertEqual(len(opener.requests), 2)
        self.assertFalse(opener.requests[1].has_header('Authorization'))

    def test_refuses_untrusted_redirects_without_sending_credentials(self):
        for url in ('https://evil.invalid/a', 'http://release-assets.githubusercontent.com/a',
                    'https://release-assets.githubusercontent.com.evil.invalid/a',
                    'https://user@release-assets.githubusercontent.com/a'):
            with self.subTest(url=url), tempfile.TemporaryDirectory() as directory:
                opener = Opener(url)
                with self.assertRaises(RuntimeError):
                    module.fetch('123', 'synthetic-test-token', Path(directory) / 'pair.tar', opener)
                self.assertEqual(len(opener.requests), 1)

    def test_draft_access_refusal_does_not_publish_or_follow(self):
        opener = Opener('https://evil.invalid/secret', 404)
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(RuntimeError, 'HTTP 404.*Keep the release draft'):
                module.fetch('123', 'synthetic-test-token', Path(directory) / 'pair.tar', opener)
        self.assertEqual(len(opener.requests), 1)

    def test_rejects_non_numeric_asset_id_before_request(self):
        opener = Opener()
        with self.assertRaises(ValueError):
            module.fetch('../releases', 'synthetic-test-token', Path('unused'), opener)
        self.assertEqual(opener.requests, [])


if __name__ == '__main__':
    unittest.main()
