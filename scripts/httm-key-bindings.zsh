# HTTM ZSH Widgets

# ALT-m - browse for ZFS snapshots interactively
httm-lookup-widget() {

  echo
  command httm -i -R

  local ret=$?
  zle reset-prompt
  return $ret

}
zle     -N      httm-lookup-widget
bindkey '\em'   httm-lookup-widget

# ALT-s - select files on ZFS snapshots interactively
__httm-select() {

  command httm -s -R | \
  while read item; do
    echo -n "${item}"
  done

  local ret=$?
  echo
  return $ret

}

httm-select-widget() {
  LBUFFER="${LBUFFER}$(__httm-select)"
  local ret=$?
  zle reset-prompt
  return $ret
}
zle     -N      httm-select-widget
bindkey '\es'   httm-select-widget