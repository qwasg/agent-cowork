from __future__ import annotations

import json
from pathlib import Path
from typing import Dict

try:
    from cryptography.fernet import Fernet
except ImportError:  # pragma: no cover
    Fernet = None  # type: ignore[assignment, misc]


class CryptoStore:
    def __init__(self, workspace_dir: str | Path | None = None) -> None:
        if workspace_dir:
            self.workspace_dir = Path(workspace_dir)
        else:
            # Default to backend/ directory (parents[4] from this file)
            self.workspace_dir = Path(__file__).resolve().parents[4]
        
        self.key_file = self.workspace_dir / ".agent_master.key"
        self.cred_file = self.workspace_dir / "agent_credentials.json"
        
        if Fernet is None:
            self._cipher = None
        else:
            self._key = self._load_or_create_key()
            self._cipher = Fernet(self._key)

    def _load_or_create_key(self) -> bytes:
        if self.key_file.exists():
            return self.key_file.read_bytes()
        
        key = Fernet.generate_key()
        try:
            self.key_file.write_bytes(key)
        except OSError:
            pass  # Fallback to ephemeral key if read-only
        return key

    def get_credentials(self) -> Dict[str, str]:
        if self._cipher is None or not self.cred_file.exists():
            return {}
        try:
            encrypted_data = self.cred_file.read_bytes()
            decrypted_data = self._cipher.decrypt(encrypted_data)
            data = json.loads(decrypted_data.decode("utf-8"))
            return data if isinstance(data, dict) else {}
        except Exception:
            return {}

    def save_credentials(self, credentials: Dict[str, str]) -> None:
        if self._cipher is None:
            raise RuntimeError("cryptography package is required to save credentials securely")
        
        data = json.dumps(credentials).encode("utf-8")
        encrypted_data = self._cipher.encrypt(data)
        self.cred_file.write_bytes(encrypted_data)
