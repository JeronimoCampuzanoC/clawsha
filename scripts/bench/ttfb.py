"""Time to the first byte of the BODY of a download, in ms, one value per line.

curl's time_starttransfer is not enough here: the gateway sends the headers
(with Content-Length) before it fetches any block, so curl would report the
time to the headers. RNF10 is about the first byte of data, which can only
leave the gateway once the whole first block has been fetched and verified.

    python3 ttfb.py GATEWAY_URL TOKEN PATH N
"""
import http.client
import sys
import time
import urllib.parse

base, token, path, n = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
url = urllib.parse.urlparse(base)
for _ in range(n):
    conn = http.client.HTTPConnection(url.hostname, url.port or 80)
    start = time.perf_counter()
    conn.request("GET", "/api/v1/files?path=" + urllib.parse.quote(path),
                 headers={"authorization": "Bearer " + token})
    resp = conn.getresponse()
    if resp.status != 200:
        sys.exit(f"{path}: HTTP {resp.status}")
    resp.read(1)
    print(f"{(time.perf_counter() - start) * 1000:.2f}")
    resp.read()
    conn.close()
