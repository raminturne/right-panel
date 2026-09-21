"""Create installable .rpp ZIPs from examples/plugins; --check validates output."""
import json
import sys
from pathlib import Path
from zipfile import ZipFile, ZIP_DEFLATED
root=Path(__file__).resolve().parents[1]; out=root/'dist'/'plugins'; out.mkdir(parents=True,exist_ok=True)
for plugin in (root/'examples'/'plugins').iterdir():
    if plugin.is_dir() and (plugin/'manifest.json').exists():
        manifest=json.loads((plugin/'manifest.json').read_text(encoding='utf-8'))
        assert manifest['id'] and manifest['entry'] and (plugin/manifest['entry']).is_file()
        assert all('..' not in Path(v).parts and not Path(v).is_absolute() for v in [manifest['entry'], manifest.get('icon','')] if v)
        with ZipFile(out/(plugin.name+'.rpp'),'w',ZIP_DEFLATED) as z:
            for p in plugin.rglob('*'):
                if p.is_file(): z.write(p,p.relative_to(plugin))
        if '--check' in sys.argv:
            with ZipFile(out/(plugin.name+'.rpp')) as z:
                assert 'manifest.json' in z.namelist() and manifest['entry'] in z.namelist()
