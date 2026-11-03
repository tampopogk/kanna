import os, secrets, subprocess, sys
user = sys.argv[1]
pw = secrets.token_hex(16)
subprocess.run(["dscl", ".", "-passwd", "/Users/" + user, pw], check=True)
subprocess.run(["dscl", ".", "-authonly", user, pw], check=True)
key = [0x7D, 0x89, 0x52, 0x23, 0xD2, 0xBC, 0xDD, 0xEA, 0xA3, 0xB9, 0x1F]
data = pw.encode() + b"\0"
data += b"\0" * (-len(data) % 12)
enc = bytes(b ^ key[i % len(key)] for i, b in enumerate(data))
if os.path.lexists("/etc/kcpassword"):
    os.unlink("/etc/kcpassword")
fd = os.open("/etc/kcpassword", os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
os.write(fd, enc)
os.close(fd)
subprocess.run(["defaults", "write", "/Library/Preferences/com.apple.loginwindow", "autoLoginUser", user], check=True)
print("autologin set for", user)
