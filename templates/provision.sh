set -eu
{% for volume in volumes %}
docker volume inspect {{ volume.name|shell_quote }} >/dev/null 2>&1 || docker volume create {{ volume.name|shell_quote }} >/dev/null
{% endfor %}
