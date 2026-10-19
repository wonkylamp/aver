// Outlives any small timeout.
console.log('sleeping');
setTimeout(() => console.log('woke up'), 10 * 60 * 1000);
