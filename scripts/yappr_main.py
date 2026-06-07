"""py2app entry point: add src/ to path and launch the yappr package."""
import os, sys
_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(_root, "src"))
from yappr.app import main
main()
