export function codeqlControl(): unknown {
  return eval(window.location.hash.slice(1));
}
